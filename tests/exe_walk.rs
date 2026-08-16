use std::fs;
use winshortcuts_organizer::icon::exe_finder::{collect_candidates, pick_executable};

mod common;

#[test]
fn walks_a_real_directory_and_picks_the_game() {
    let tmp = common::unique_temp_dir("exe_walk_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("bin").join("win64")).unwrap();
    fs::write(tmp.join("gmod.exe"), vec![0u8; 400]).unwrap();
    fs::write(tmp.join("bin").join("win64").join("studiomdl.exe"), vec![0u8; 4000]).unwrap();
    fs::write(tmp.join("readme.txt"), b"not an exe").unwrap();

    let cands = collect_candidates(&tmp, 4);
    assert_eq!(cands.len(), 2, "only .exe files are collected");

    let picked = pick_executable("Garry's Mod", "GarrysMod", &cands).unwrap();
    assert_eq!(picked.rel_path.file_name().unwrap(), "gmod.exe");

    fs::remove_dir_all(&tmp).unwrap();
}
