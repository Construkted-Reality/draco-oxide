use super::rans;
use crate::core::bit_coder::BitWriter;
use crate::core::buffer::LsbFirst;
use crate::encode::entropy::rans::RansSymbolEncoder;
use crate::prelude::ByteWriter;
use crate::shared::entropy::SymbolEncodingMethod;

#[derive(thiserror::Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Err {
    #[error("RANS encoding error")]
    RansEncodingError(#[from] rans::Err),
    #[error("Invalid inputs for encode_tagged_symbol(): It must be true that symbol.len()==num_values*num_components, but got symbol.len()={0}, num_values={1}, num_components={2}")]
    InvalidInputs(usize, usize, usize),
    #[error("Invalid bit length: {0}")]
    InvalidBitLength(usize),
}

/// Encodes a stream of symbols, choosing between the tagged (length-coded) and
/// raw (direct) rANS schemes the way Google's `EncodeSymbols` does
/// (`compression/entropy/symbol_encoding.cc`): estimate the bit cost of each
/// scheme and emit the smaller, forcing tagged when the raw value range exceeds
/// 18 bits. Pass `method_override = Some(..)` to force a specific scheme (mirrors
/// Google's `symbol_encoding_method` option; used by the connectivity coder and
/// the round-trip unit tests). The selected method is written into the bitstream
/// as a single byte that the decoder reads back.
pub fn encode_symbols<W>(
    symbols: Vec<u64>,
    num_components: usize,
    method_override: Option<SymbolEncodingMethod>,
    writer: &mut W,
) -> Result<(), Err>
where
    W: ByteWriter,
{
    let (bit_lengths, max_value) = compute_bit_lengths(&symbols, num_components);
    let method = method_override.unwrap_or_else(|| {
        select_symbol_encoding_method(&symbols, &bit_lengths, max_value, num_components)
    });

    method.write_to(writer);
    match method {
        SymbolEncodingMethod::LengthCoded => {
            let bit_lengths_u8 = bit_lengths.iter().map(|&b| b as u8).collect();
            encode_symbols_length_coded(symbols, num_components, bit_lengths_u8, writer)
        }
        SymbolEncodingMethod::DirectCoded => {
            let num_unique_symbols = count_unique_symbols(&symbols);
            encode_symbols_direct_coded(symbols, num_unique_symbols, writer)
        }
    }
}

/// 0-indexed position of the most significant set bit. Caller guarantees `n > 0`.
/// Equivalent to Google's `MostSignificantBit` (`core/bit_utils.h`).
fn most_significant_bit(n: u64) -> u32 {
    63 - n.leading_zeros()
}

/// Per-value bit lengths used by the tagged scheme, plus the maximum symbol
/// value across all components. Mirrors Google's `ComputeBitLengths`: for each
/// `num_components`-sized chunk it takes the largest component and stores
/// `MostSignificantBit(value) + 1` (0 stored as 0).
fn compute_bit_lengths(symbols: &[u64], num_components: usize) -> (Vec<u32>, u64) {
    let nc = num_components.max(1);
    let mut bit_lengths = Vec::with_capacity(symbols.len() / nc);
    let mut max_value = 0u64;
    let mut i = 0;
    while i < symbols.len() {
        let mut max_component = symbols[i];
        for j in 1..nc {
            if i + j < symbols.len() && symbols[i + j] > max_component {
                max_component = symbols[i + j];
            }
        }
        let value_msb_pos = if max_component > 0 {
            most_significant_bit(max_component)
        } else {
            0
        };
        if max_component > max_value {
            max_value = max_component;
        }
        bit_lengths.push(value_msb_pos + 1);
        i += nc;
    }
    (bit_lengths, max_value)
}

/// Number of distinct symbol values (Google's `num_unique_symbols`). NOTE: this
/// is the count of *distinct values*, not the count of non-zero entries — the
/// latter was a bug that inflated the rANS precision.
fn count_unique_symbols(symbols: &[u64]) -> usize {
    symbols
        .iter()
        .copied()
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Approximate Shannon entropy in bits, plus the number of distinct symbols.
/// Mirrors `ComputeShannonEntropy` (`compression/entropy/shannon_entropy.cc`).
fn compute_shannon_entropy(symbols: &[u64], max_value: u64) -> (i64, i64) {
    let mut freq = vec![0i64; max_value as usize + 1];
    for &s in symbols {
        freq[s as usize] += 1;
    }
    let n = symbols.len() as f64;
    let mut total_bits = 0.0f64;
    let mut num_unique = 0i64;
    for &f in &freq {
        if f > 0 {
            num_unique += 1;
            total_bits += f as f64 * ((f as f64) / n).log2();
        }
    }
    ((-total_bits) as i64, num_unique)
}

/// Approximate size (bits) of the serialized rANS frequency table. Mirrors
/// `ApproximateRAnsFrequencyTableBits` (`compression/entropy/rans_symbol_coding.h`).
fn approximate_rans_frequency_table_bits(max_value: i64, num_unique_symbols: i64) -> i64 {
    let table_zero_frequency_bits =
        8 * (num_unique_symbols + (max_value - num_unique_symbols) / 64);
    8 * num_unique_symbols + table_zero_frequency_bits
}

/// Approximate bit cost of the tagged scheme. Mirrors `ApproximateTaggedSchemeBits`.
fn approximate_tagged_scheme_bits(bit_lengths: &[u32], num_components: usize) -> i64 {
    let total_bit_length: i64 = bit_lengths.iter().map(|&b| b as i64).sum();
    let bl: Vec<u64> = bit_lengths.iter().map(|&b| b as u64).collect();
    // Google fixes max_value = 32 (kMaxTagSymbolBitLength) for the tag entropy.
    let (tag_bits, num_unique) = compute_shannon_entropy(&bl, 32);
    let tag_table_bits = approximate_rans_frequency_table_bits(num_unique, num_unique);
    tag_bits + tag_table_bits + total_bit_length * num_components as i64
}

/// Approximate bit cost of the raw scheme, plus the distinct-symbol count.
/// Mirrors `ApproximateRawSchemeBits`.
fn approximate_raw_scheme_bits(symbols: &[u64], max_value: u64) -> (i64, i64) {
    let (data_bits, num_unique) = compute_shannon_entropy(symbols, max_value);
    let table_bits = approximate_rans_frequency_table_bits(max_value as i64, num_unique);
    (table_bits + data_bits, num_unique)
}

/// Chooses tagged vs raw exactly as Google's `EncodeSymbols`: tagged when its
/// estimate is smaller, or when the raw value range exceeds 18 bits.
fn select_symbol_encoding_method(
    symbols: &[u64],
    bit_lengths: &[u32],
    max_value: u64,
    num_components: usize,
) -> SymbolEncodingMethod {
    if symbols.is_empty() {
        return SymbolEncodingMethod::DirectCoded;
    }
    let tagged_bits = approximate_tagged_scheme_bits(bit_lengths, num_components);
    let (raw_bits, _num_unique) = approximate_raw_scheme_bits(symbols, max_value);
    let max_value_bit_length = most_significant_bit(max_value.max(1)) as i64 + 1;
    if tagged_bits < raw_bits || max_value_bit_length > 18 {
        SymbolEncodingMethod::LengthCoded // SYMBOL_CODING_TAGGED
    } else {
        SymbolEncodingMethod::DirectCoded // SYMBOL_CODING_RAW
    }
}

/// Encodes symbols using the rANS coder as the tag encoder, that is, the symbols are encoded as bits, and the
/// bit lengths are encoded by the rANS coder.
///     symbols: the symbols to encode. For data with multiple components (e.g., 3D points are with 3 components), \
///        the symbols must be a vector of length `num_values * num_components` (e.g. a set of 100 3D points is\
///         represented as 300 symbols).
///     num_components: the number of components for each value (e.g., 3 for 3D points).
///     bit_lengths: the bit lengths of the symbols. It is a vector of 'symbols.len()/num_components' elements, and\
///         records the largest bit length of the 'num_components' components.
///     writer: byte writer
fn encode_symbols_length_coded<W>(
    symbols: Vec<u64>,
    num_components: usize,
    bit_lengths: Vec<u8>,
    writer: &mut W,
) -> Result<(), Err>
where
    W: ByteWriter,
{
    let mut freq_counts = Vec::new();

    for &bit_length in &bit_lengths {
        let bit_length = bit_length as usize;
        if freq_counts.len() <= bit_length {
            freq_counts.resize(bit_length + 1, 0);
        }
        freq_counts[bit_length] += 1;
    }

    let mut values = Vec::new();
    let mut encoder = RansSymbolEncoder::<'_, _, 5, 12>::new(writer, freq_counts, None)?;
    for i in (0..symbols.len() / num_components).rev() {
        let bit_length = bit_lengths[i] as usize;
        encoder.write(bit_length)?;

        // Values are always encoded in the normal order
        let j = symbols.len() - num_components - i * num_components;
        let value_bit_length = bit_lengths[j / num_components];
        for c in 0..num_components {
            values.push((value_bit_length, symbols[j + c]));
        }
    }
    encoder.flush()?;

    // Append the values to the end of the target buffer.
    // Google's decoder reads these via `DecodeLeastSignificantBits32`
    // (`compression/entropy/symbol_decoding.cc`), so encode LSB-first.
    let mut writer: BitWriter<_, LsbFirst> = BitWriter::spown_from(writer);
    for val in values.into_iter() {
        writer.write_bits(val);
    }
    Ok(())
}

fn encode_symbols_direct_coded<W>(
    symbols: Vec<u64>,
    num_unique_symbols: usize,
    writer: &mut W,
) -> Result<(), Err>
where
    W: ByteWriter,
{
    // unique_symbols_bit_length = MostSignificantBit(num_unique) + 1, clamped to
    // [1, 18]. (Google also applies a compression-level adjustment, but it is a
    // no-op at the default level 7, which is what we encode at.) The previous
    // implementation used `+ 1` too many (MSB + 2) and fed it the non-zero count
    // rather than the distinct-symbol count, both of which inflated the rANS
    // precision and the serialized frequency table.
    let bit_length = if num_unique_symbols == 0 {
        1
    } else {
        (64 - num_unique_symbols.leading_zeros() as usize).clamp(1, 18)
    };
    writer.write_u8(bit_length as u8);
    match bit_length {
        1 => encode_symbols_direct_coded_precision_unwrapped::<W, 1, 12>(symbols, writer),
        2 => encode_symbols_direct_coded_precision_unwrapped::<W, 2, 12>(symbols, writer),
        3 => encode_symbols_direct_coded_precision_unwrapped::<W, 3, 12>(symbols, writer),
        4 => encode_symbols_direct_coded_precision_unwrapped::<W, 4, 12>(symbols, writer),
        5 => encode_symbols_direct_coded_precision_unwrapped::<W, 5, 12>(symbols, writer),
        6 => encode_symbols_direct_coded_precision_unwrapped::<W, 6, 12>(symbols, writer),
        7 => encode_symbols_direct_coded_precision_unwrapped::<W, 7, 12>(symbols, writer),
        8 => encode_symbols_direct_coded_precision_unwrapped::<W, 8, 12>(symbols, writer),
        9 => encode_symbols_direct_coded_precision_unwrapped::<W, 9, 13>(symbols, writer),
        10 => encode_symbols_direct_coded_precision_unwrapped::<W, 10, 15>(symbols, writer),
        11 => encode_symbols_direct_coded_precision_unwrapped::<W, 11, 16>(symbols, writer),
        12 => encode_symbols_direct_coded_precision_unwrapped::<W, 12, 18>(symbols, writer),
        13 => encode_symbols_direct_coded_precision_unwrapped::<W, 13, 19>(symbols, writer),
        14 => encode_symbols_direct_coded_precision_unwrapped::<W, 14, 20>(symbols, writer),
        15 => encode_symbols_direct_coded_precision_unwrapped::<W, 15, 20>(symbols, writer),
        16 => encode_symbols_direct_coded_precision_unwrapped::<W, 16, 20>(symbols, writer),
        17 => encode_symbols_direct_coded_precision_unwrapped::<W, 17, 20>(symbols, writer),
        18 => encode_symbols_direct_coded_precision_unwrapped::<W, 18, 20>(symbols, writer),
        _ => unreachable!("This should never happen, as the  bit length is clamped to a minimum of 1 and a maximum of 18"),
    }
}

fn encode_symbols_direct_coded_precision_unwrapped<
    W,
    const NUM_SYMBOLS_BIT_LENGTH: usize,
    const RANS_PRECISION: usize,
>(
    symbols: Vec<u64>,
    writer: &mut W,
) -> Result<(), Err>
where
    W: ByteWriter,
{
    let mut freq_counts = Vec::with_capacity(symbols.len());
    let mut max_symbol = 0;
    for &s in symbols.iter() {
        if s >= max_symbol {
            max_symbol = s;
            freq_counts.resize((max_symbol + 1) as usize, 0);
        }
        freq_counts[s as usize] += 1;
    }

    let mut encoder = RansSymbolEncoder::<'_, _, NUM_SYMBOLS_BIT_LENGTH, RANS_PRECISION>::new(
        writer,
        freq_counts,
        None,
    )?;

    for s in symbols.into_iter().rev() {
        encoder.write(s as usize)?;
    }
    encoder.flush()?;
    Ok(())
}
