use draco_oxide::eval::EvalWriter;
use draco_oxide::io::obj::load_obj;
use draco_oxide::prelude::*;
use std::io::Write;

const MESH_NAME: &str = "tetrahedron";

#[test]
fn test_eval() {
    let original_mesh = load_obj(format!("tests/data/{}.obj", MESH_NAME)).unwrap();

    let mut buffer = Vec::new();
    let mut writer = EvalWriter::new(&mut buffer);
    encode(
        original_mesh.clone(),
        &mut writer,
        encode::Config::default(),
    )
    .unwrap();

    // Write the evaluation data to a temp file (test should not depend on
    // a checked-out tests/outputs/ directory).
    let json = writer.get_result();
    let json = serde_json::to_string_pretty(&json).unwrap();
    let eval_output_path = std::env::temp_dir().join(format!("draco_oxide_{}_eval.txt", MESH_NAME));
    let mut eval_file =
        std::fs::File::create(&eval_output_path).expect("Failed to create evaluation output file");
    eval_file
        .write_all(json.as_bytes())
        .expect("Failed to write evaluation data");

    let output_path = std::env::temp_dir().join(format!("draco_oxide_{}_eval.drc", MESH_NAME));
    let mut file = std::fs::File::create(&output_path).expect("Failed to create output file");
    file.write_all(&buffer)
        .expect("Failed to write encoded data");

    let _ = std::fs::remove_file(&eval_output_path);
    let _ = std::fs::remove_file(&output_path);
}
