use std::fs;
use std::path::PathBuf;
use winshortcuts_organizer::platform::epic::{launch_uri, EpicProvider};
use winshortcuts_organizer::platform::GameProvider;

mod common;

#[test]
fn reads_manifests_and_filters_non_games() {
    let tmp = common::unique_temp_dir("epic_provider_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    fs::write(
        tmp.join("game.item"),
        r#"{"DisplayName":"Fall Guys","InstallLocation":"D:\\Epic\\FallGuys",
            "LaunchExecutable":"FallGuys_client.exe","AppName":"9c88",
            "CatalogNamespace":"0a2d9f","CatalogItemId":"abc123",
            "AppCategories":["public","games"]}"#,
    )
    .unwrap();
    fs::write(
        tmp.join("dlc.item"),
        r#"{"DisplayName":"Addon","InstallLocation":"D:\\Epic\\Addon",
            "LaunchExecutable":"a.exe","AppName":"a","CatalogNamespace":"n",
            "CatalogItemId":"c","AppCategories":["addons"]}"#,
    )
    .unwrap();
    fs::write(tmp.join("notes.txt"), "ignored").unwrap();

    let games = EpicProvider::from_dirs(tmp.clone(), PathBuf::from("launcher.exe"))
        .games()
        .unwrap();

    assert_eq!(games.len(), 1);
    assert_eq!(games[0].name, "Fall Guys");
    assert_eq!(
        games[0].launch.args,
        format!("\"{}\"", launch_uri("0a2d9f", "abc123", "9c88"))
    );
    assert!(games[0].launch.program.ends_with("explorer.exe"));

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn skips_malformed_manifest() {
    let tmp = common::unique_temp_dir("epic_provider_malformed_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    // Missing required field (CatalogItemId)
    fs::write(
        tmp.join("malformed.item"),
        r#"{"DisplayName":"Bad Manifest","InstallLocation":"D:\\Epic\\Bad",
            "LaunchExecutable":"bad.exe","AppName":"bad",
            "CatalogNamespace":"n","AppCategories":["games"]}"#,
    )
    .unwrap();

    let games = EpicProvider::from_dirs(tmp.clone(), PathBuf::from("launcher.exe"))
        .games()
        .unwrap();

    assert_eq!(games.len(), 0);

    fs::remove_dir_all(&tmp).unwrap();
}
