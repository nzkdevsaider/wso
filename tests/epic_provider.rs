use std::fs;
use std::path::PathBuf;
use winshortcuts_organizer::platform::{epic::EpicProvider, GameProvider};

#[test]
fn reads_manifests_and_filters_non_games() {
    let tmp = std::env::temp_dir().join("wso_epic_provider_test");
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
    assert!(games[0].launch.args.contains("0a2d9f%3Aabc123%3A9c88"));
    assert!(games[0].launch.program.ends_with("explorer.exe"));

    fs::remove_dir_all(&tmp).unwrap();
}
