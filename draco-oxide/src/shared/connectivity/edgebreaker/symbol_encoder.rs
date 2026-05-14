use crate::core::bit_coder::BitReader;
use crate::core::buffer::OrderConfig;
use crate::encode::connectivity::edgebreaker::Err;
use crate::prelude::ByteReader;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Symbol {
    C,
    S,
    L,
    R,
    E,
}

impl Symbol {
    #[inline]
    /// Returns the symbol as a character together with the metadata if it is a hole or handle.
    #[allow(unused)] // May be used in the future for debugging or logging.
    pub(crate) fn as_char(&self) -> (char, Option<usize>) {
        match self {
            Symbol::C => ('C', None),
            Symbol::R => ('R', None),
            Symbol::L => ('L', None),
            Symbol::E => ('E', None),
            Symbol::S => ('S', None),
        }
    }

    /// Returns the symbol id of the symbol.
    /// This id must be compatible with the draco library.
    pub(crate) fn get_id(self) -> usize {
        match self {
            Symbol::C => 0,
            Symbol::S => 1,
            Symbol::L => 2,
            Symbol::R => 3,
            Symbol::E => 4,
        }
    }
}

pub(crate) trait SymbolEncoder {
    fn encode_symbol(symbol: Symbol) -> Result<(u8, u64), Err>;

    /// Decodes one CrLight-encoded symbol from `reader`. Parameterized over
    /// `OrderConfig` because the encoder writes via `BitWriter<_, LsbFirst>`
    /// (see `encode/connectivity/edgebreaker.rs:657`), so the matching
    /// reader must also use `LsbFirst`.
    fn decode_symbol<R, O>(reader: &mut BitReader<R, O>) -> Symbol
    where
        R: ByteReader,
        O: OrderConfig;
}

pub(crate) struct CrLight;
impl SymbolEncoder for CrLight {
    fn encode_symbol(symbol: Symbol) -> Result<(u8, u64), Err> {
        // CrLight prefix code (LsbFirst on the wire):
        //   C : "0"
        //   S : "1 0 0"   (value 0b001 written 3 bits LsbFirst)
        //   L : "1 1 0"   (value 0b011)
        //   R : "1 0 1"   (value 0b101)
        //   E : "1 1 1"   (value 0b111)
        match symbol {
            Symbol::C => Ok((1, 0)),
            Symbol::S => Ok((3, 0b001)),
            Symbol::L => Ok((3, 0b011)),
            Symbol::R => Ok((3, 0b101)),
            Symbol::E => Ok((3, 0b111)),
        }
    }

    fn decode_symbol<R, O>(reader: &mut BitReader<R, O>) -> Symbol
    where
        R: ByteReader,
        O: OrderConfig,
    {
        // LsbFirst: bit 0 is the lowest-significance bit of `value`. So
        // after reading the leading "1", a `read_bits(2)` returns the
        // next two bits packed as `bit_pos1 << 0 | bit_pos2 << 1`.
        //
        // Mapping from `read_bits(2)` to symbol (consistent with
        // `encode_symbol` LsbFirst values >> 1):
        //   value 0b00 (raw bits 0,0) -> S    (encode value 0b001)
        //   value 0b01 (raw bits 1,0) -> L    (encode value 0b011)
        //   value 0b10 (raw bits 0,1) -> R    (encode value 0b101)
        //   value 0b11 (raw bits 1,1) -> E    (encode value 0b111)
        if reader.read_bits(1).unwrap() == 0 {
            return Symbol::C;
        }
        match reader.read_bits(2).unwrap() {
            0b00 => Symbol::S,
            0b01 => Symbol::L,
            0b10 => Symbol::R,
            0b11 => Symbol::E,
            _ => unreachable!("read_bits(2) returns at most 0b11"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::bit_coder::{BitReader, BitWriter};
    use crate::core::buffer::LsbFirst;

    fn round_trip(symbols: &[Symbol]) {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut writer: BitWriter<'_, Vec<u8>, LsbFirst> = BitWriter::spown_from(&mut buf);
            for &s in symbols {
                writer.write_bits(CrLight::encode_symbol(s).unwrap());
            }
        }

        let mut iter = buf.into_iter();
        let mut reader: BitReader<'_, _, LsbFirst> = BitReader::spown_from(&mut iter).unwrap();
        let decoded: Vec<Symbol> = (0..symbols.len())
            .map(|_| CrLight::decode_symbol(&mut reader))
            .collect();
        assert_eq!(decoded, symbols);
    }

    #[test]
    fn cr_light_each_symbol_round_trips() {
        round_trip(&[Symbol::C]);
        round_trip(&[Symbol::S]);
        round_trip(&[Symbol::L]);
        round_trip(&[Symbol::R]);
        round_trip(&[Symbol::E]);
    }

    #[test]
    fn cr_light_mixed_sequence_round_trips() {
        round_trip(&[
            Symbol::C,
            Symbol::R,
            Symbol::C,
            Symbol::E,
            Symbol::L,
            Symbol::S,
            Symbol::C,
            Symbol::C,
            Symbol::R,
            Symbol::L,
            Symbol::E,
            Symbol::S,
        ]);
    }

    #[test]
    fn cr_light_long_sequence_round_trips() {
        // Force multi-byte boundaries.
        let pattern = [
            Symbol::C,
            Symbol::C,
            Symbol::R,
            Symbol::L,
            Symbol::S,
            Symbol::E,
            Symbol::C,
            Symbol::R,
        ];
        let long: Vec<Symbol> = pattern.iter().cloned().cycle().take(200).collect();
        round_trip(&long);
    }
}
