use std::fs;
use std::path::PathBuf;
use winshortcuts_organizer::platform::{steam::SteamProvider, GameProvider};

fn write(path: PathBuf, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn discovers_games_and_filters_infrastructure() {
    let tmp = std::env::temp_dir().join("wso_steam_provider_test");
    let _ = fs::remove_dir_all(&tmp);
    let root = tmp.join("Steam");

    write(
        root.join("steamapps").join("libraryfolders.vdf"),
        &format!(
            "\"libraryfolders\"\n{{\n\"0\"\n{{\n\"path\"\t\t\"{}\"\n}}\n}}\n",
            root.to_string_lossy().replace('\\', "\\\\")
        ),
    );
    write(
        root.join("steamapps").join("appmanifest_105600.acf"),
        "\"AppState\"\n{\n\"appid\"\t\"105600\"\n\"name\"\t\"Terraria\"\n\"installdir\"\t\"Terraria\"\n}\n",
    );
    write(
        root.join("steamapps").join("appmanifest_228980.acf"),
        "\"AppState\"\n{\n\"appid\"\t\"228980\"\n\"name\"\t\"Steamworks Common Redistributables\"\n\"installdir\"\t\"Steamworks Shared\"\n}\n",
    );
    fs::create_dir_all(root.join("steamapps").join("common").join("Terraria")).unwrap();
    fs::create_dir_all(root.join("steamapps").join("common").join("Steamworks Shared")).unwrap();
    fs::write(
        root.join("steamapps").join("common").join("Terraria").join("Terraria.exe"),
        vec![0u8; 100],
    )
    .unwrap();

    let games = SteamProvider::from_root(root.clone()).games().unwrap();

    assert_eq!(games.len(), 1, "infrastructure appid must be filtered out");
    assert_eq!(games[0].name, "Terraria");
    assert_eq!(games[0].id, "105600");
    assert_eq!(games[0].launch.args, "-applaunch 105600");
    assert_eq!(games[0].launch.program, root.join("steam.exe"));
    assert_eq!(games[0].icon_candidates.len(), 3, "exe, librarycache, fallback");

    fs::remove_dir_all(&tmp).unwrap();
}
