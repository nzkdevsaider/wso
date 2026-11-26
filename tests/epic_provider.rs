use std::fs;
use std::path::PathBuf;
use winshortcutsorganizer::platform::GameProvider;
use winshortcutsorganizer::platform::epic::{EpicProvider, launch_uri};

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

#[test]
fn reads_preview_installations_jsonl_and_caches() {
    let tmp = common::unique_temp_dir("epic_preview_provider_test");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).unwrap();

    let games_root = tmp.join("D_EpicGames");
    let btd_dir = games_root.join("BloonsTD6");
    let fn_dir = games_root.join("Fortnite");
    let ue_dir = games_root.join("UE_5.5");
    fs::create_dir_all(&btd_dir).unwrap();
    fs::create_dir_all(&fn_dir).unwrap();
    fs::create_dir_all(&ue_dir).unwrap();

    let jsonl = format!(
        r#"{{"version":1,"writtenAt":"2026-11-25T02:03:09Z","entries":4}}
{{"installationId":"ID_BTD","complete":true,"dir":{btd:?},"platform":"Windows","namespace":"6a8d","itemId":"item_btd","artifactId":"art_btd","version":"57.4","launchExecutable":"BloonsTD6.exe"}}
{{"installationId":"ID_FN","complete":false,"dir":{fn_dir:?},"platform":"Windows","namespace":"fn","itemId":"item_fn","artifactId":"Fortnite","version":"43.10","pendingVersion":"43.11","launchExecutable":"FortniteGame/Binaries/Win64/FortniteBootstrapper.exe"}}
{{"installationId":"ID_UE","complete":true,"dir":{ue_dir:?},"platform":"Windows","namespace":"ue","itemId":"item_ue","artifactId":"UE_5.5","version":"5.5.0","launchExecutable":"Engine/Binaries/Win64/UnrealEditor.exe"}}
{{"installationId":"ID_MISSING_DIR","complete":true,"dir":{missing:?},"platform":"Windows","namespace":"ns","itemId":"item_missing","artifactId":"art_missing","version":"1.0","launchExecutable":"game.exe"}}
"#,
        btd = btd_dir.to_str().unwrap(),
        fn_dir = fn_dir.to_str().unwrap(),
        ue_dir = ue_dir.to_str().unwrap(),
        missing = games_root.join("DeletedOnDisk").to_str().unwrap(),
    );
    fs::write(tmp.join("installations.jsonl"), jsonl).unwrap();

    let account_dir = tmp.join("cache").join("prod").join("account").join("acc1");
    fs::create_dir_all(&account_dir).unwrap();
    fs::write(
        account_dir.join("library-cache.json"),
        r#"{"items":[
            {"appId":{"namespaceId":"6a8d","itemId":"item_btd","appName":"art_btd"},"title":"Bloons TD 6"},
            {"appId":{"namespaceId":"fn","itemId":"item_fn","appName":"Fortnite"},"title":"Fortnite"}
        ]}"#,
    )
    .unwrap();

    fs::write(
        tmp.join("cache").join("prod").join("catalog-cache.json"),
        r#"{"items":{
            "item_ue":{"item":{"id":"item_ue","title":"Unreal Engine","categories":[{"path":"engines/ue5"},{"path":"engines"}]}}
        }}"#,
    )
    .unwrap();

    let games = EpicProvider::from_preview_dir(tmp.clone(), PathBuf::from("launcher.exe"))
        .games()
        .unwrap();

    assert_eq!(games.len(), 2, "expected 2 games, got {games:?}");
    assert_eq!(games[0].name, "Bloons TD 6");
    assert_eq!(games[0].id, "art_btd");
    assert_eq!(
        games[0].launch.args,
        format!("\"{}\"", launch_uri("6a8d", "item_btd", "art_btd"))
    );
    assert_eq!(games[1].name, "Fortnite");
    assert_eq!(games[1].id, "Fortnite");

    fs::remove_dir_all(&tmp).unwrap();
}

#[test]
fn deduplicates_games_present_in_both_preview_and_legacy_manifests() {
    let tmp = common::unique_temp_dir("epic_both_dirs_dedup");
    let _ = fs::remove_dir_all(&tmp);
    let legacy_dir = tmp.join("legacy");
    let preview_dir = tmp.join("preview");
    let fn_dir = tmp.join("Fortnite");
    fs::create_dir_all(&legacy_dir).unwrap();
    fs::create_dir_all(&preview_dir).unwrap();
    fs::create_dir_all(&fn_dir).unwrap();

    fs::write(
        legacy_dir.join("fn.item"),
        format!(
            r#"{{"DisplayName":"Fortnite","InstallLocation":{fn_dir:?},
                "LaunchExecutable":"FortniteBootstrapper.exe","AppName":"Fortnite",
                "CatalogNamespace":"fn","CatalogItemId":"4fe7",
                "AppCategories":["games"]}}"#,
            fn_dir = fn_dir.to_str().unwrap()
        ),
    )
    .unwrap();

    fs::write(
        preview_dir.join("installations.jsonl"),
        format!(
            r#"{{"version":1}}
{{"installationId":"ID_FN","complete":true,"dir":{fn_dir:?},"platform":"Windows","namespace":"fn","itemId":"4fe7","artifactId":"Fortnite","version":"43.10","launchExecutable":"FortniteBootstrapper.exe"}}
"#,
            fn_dir = fn_dir.to_str().unwrap()
        ),
    )
    .unwrap();

    let games =
        EpicProvider::from_both_dirs(legacy_dir, preview_dir, PathBuf::from("launcher.exe"))
            .games()
            .unwrap();

    assert_eq!(games.len(), 1);
    assert_eq!(games[0].name, "Fortnite");

    fs::remove_dir_all(&tmp).unwrap();
}
