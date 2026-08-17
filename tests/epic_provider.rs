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

/// Windows filenames are case-insensitive, so a manifest saved as
/// `game.ITEM` is exactly as real as `game.item`. Matching the extension
/// strictly made such a file invisible: the game was never detected at all,
/// with no error and nothing to explain the absence — the worst shape a
/// failure can take, because there is nothing for the user to act on.
///
/// The lowercase sibling in the same directory is what makes this a test of
/// the extension match rather than of the whole provider: if the uppercase
/// file were ignored, one game would still be found and only the count would
/// betray it.
#[test]
fn reads_a_manifest_saved_with_an_uppercase_extension() {
    let tmp = common::unique_temp_dir("epic_provider_uppercase_extension");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    fs::write(
        tmp.join("shouty.ITEM"),
        r#"{"DisplayName":"Fall Guys","InstallLocation":"D:\\Epic\\FallGuys",
            "LaunchExecutable":"FallGuys_client.exe","AppName":"9c88",
            "CatalogNamespace":"0a2d9f","CatalogItemId":"abc123",
            "AppCategories":["games"]}"#,
    )
    .unwrap();
    fs::write(
        tmp.join("quiet.item"),
        r#"{"DisplayName":"Rocket League","InstallLocation":"D:\\Epic\\RL",
            "LaunchExecutable":"rl.exe","AppName":"rl","CatalogNamespace":"n",
            "CatalogItemId":"c","AppCategories":["games"]}"#,
    )
    .unwrap();

    let games = EpicProvider::from_dirs(tmp.clone(), PathBuf::from("launcher.exe"))
        .games()
        .unwrap();

    assert_eq!(games.len(), 2, "both manifests must be read: {games:?}");
    assert!(
        games.iter().any(|g| g.name == "Fall Guys"),
        "the .ITEM manifest must be read: {games:?}"
    );

    fs::remove_dir_all(&tmp).unwrap();
}
