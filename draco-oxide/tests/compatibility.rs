use draco_oxide::prelude::ConfigType;
use draco_oxide::{
    encode::{self, encode},
    io::obj::load_obj,
};
use std::io::Write;

const FILE_NAME: &str = "cube_quads";

#[test]
fn en() {
    let mesh = load_obj(format!("tests/data/{}.obj", FILE_NAME)).unwrap();

    let mut writer = Vec::new();
    encode(mesh.clone(), &mut writer, encode::Config::default()).unwrap();

    // Write to a temp file so the test doesn't depend on a checked-out
    // `tests/outputs/` directory.
    let out = std::env::temp_dir().join(format!("draco_oxide_{}.drc", FILE_NAME));
    let mut file = std::fs::File::create(&out).unwrap();

    file.write_all(&writer).unwrap();
    let _ = std::fs::remove_file(&out);
}
