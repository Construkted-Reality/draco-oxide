use draco_oxide::io::gltf::GltfTranscoder;
use std::path::Path;

fn main() {
    // input file
    let input_path = "input.glb";

    // output file
    let output_path = "output.glb";

    // Read input file
    let input = std::fs::read(input_path).expect("Failed to read input file");

    // Create transcoder with default options
    let transcoder = GltfTranscoder::default();

    // Transcode and write to file
    let warnings = transcoder
        .transcode_to_file(&input, Path::new(output_path))
        .expect("Transcoding failed");

    // Print any warnings
    for warning in warnings {
        println!("Warning: {}", warning);
    }

    println!("Transcoding complete: {} -> {}", input_path, output_path);
}
