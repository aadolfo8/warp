use super::*;

fn icon(path: &str) -> &'static str {
    file_icon(path)
}

#[test]
fn files_match_by_name_before_extension() {
    assert_eq!(icon("package.json"), "bundled/svg/material/nodejs.svg");
    assert_eq!(icon("tsconfig.json"), "bundled/svg/material/tsconfig.svg");
    assert_eq!(icon("Dockerfile"), "bundled/svg/material/docker.svg");
    assert_eq!(icon("README.md"), "bundled/svg/material/readme.svg");
    assert_eq!(icon("other.json"), "bundled/svg/material/json.svg");
}

#[test]
fn files_prefer_the_longest_extension() {
    assert_eq!(
        icon("src/users/users.controller.ts"),
        "bundled/svg/material/controller.svg"
    );
    assert_eq!(
        icon("app.service.spec.ts"),
        "bundled/svg/material/test-ts.svg"
    );
    assert_eq!(icon("main.ts"), "bundled/svg/material/typescript.svg");
}

#[test]
fn dotfiles_and_windows_paths_match() {
    assert_eq!(icon(".env"), "bundled/svg/material/tune.svg");
    assert_eq!(icon(".gitignore"), "bundled/svg/material/git.svg");
    assert_eq!(
        icon(r"C:\proyecto\src\Main.PY"),
        "bundled/svg/material/python.svg"
    );
}

#[test]
fn unknown_files_get_the_default_icon() {
    assert_eq!(icon("notes.zzz-unknown"), "bundled/svg/material/file.svg");
    assert_eq!(icon("LICENSE-ish-thing"), "bundled/svg/material/file.svg");
}

#[test]
fn folders_have_named_open_and_closed_icons() {
    assert_eq!(
        folder_icon("project/src", false, false),
        "bundled/svg/material/folder-src.svg"
    );
    assert_eq!(
        folder_icon("project/src/", true, false),
        "bundled/svg/material/folder-src-open.svg"
    );
    assert_eq!(
        folder_icon("node_modules", false, false),
        "bundled/svg/material/folder-node.svg"
    );
    assert_eq!(
        folder_icon("something-else", false, false),
        "bundled/svg/material/folder.svg"
    );
    assert_eq!(
        folder_icon("something-else", true, false),
        "bundled/svg/material/folder-open.svg"
    );
    assert_eq!(
        folder_icon("logibackend", false, true),
        "bundled/svg/material/folder-root.svg"
    );
}

#[test]
fn every_manifest_icon_is_bundled() {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let icons = icons();
    let all = icons
        .file_names
        .values()
        .chain(icons.file_extensions.values())
        .copied()
        .chain(
            icons
                .folder_names
                .values()
                .flat_map(|(closed, open)| [*closed, *open]),
        )
        .chain([
            icons.file,
            icons.folder,
            icons.folder_expanded,
            icons.root_folder,
            icons.root_folder_expanded,
        ]);
    for path in all {
        assert!(assets.join(path).is_file(), "missing bundled icon {path}");
    }
}
