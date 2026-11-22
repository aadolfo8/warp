use pathfinder_color::ColorU;

use super::*;

#[test]
fn strip_jsonc_removes_comments_and_trailing_commas() {
    let text = r#"{
        // line comment
        "a": "http://x", /* block */
        "b": [1, 2,],
    }"#;
    let value: Value = serde_json::from_str(&strip_jsonc(text)).expect("valid json");
    assert_eq!(value["a"], "http://x");
    assert_eq!(value["b"], serde_json::json!([1, 2]));
}

#[test]
fn parse_color_handles_short_and_alpha_forms() {
    assert_eq!(parse_color("#0a0a0d"), Some(ColorU::new(10, 10, 13, 255)));
    assert_eq!(parse_color("#515c7e40"), Some(ColorU::new(81, 92, 126, 64)));
    assert_eq!(parse_color("#fff"), Some(ColorU::new(255, 255, 255, 255)));
    assert_eq!(parse_color("red"), None);
}

#[test]
fn scope_color_prefers_the_most_specific_selector() {
    let theme = Theme {
        colors: HashMap::new(),
        token_rules: vec![
            (vec!["keyword".to_owned()], "#111111".to_owned()),
            (
                vec!["keyword.control.import".to_owned()],
                "#222222".to_owned(),
            ),
            (vec!["keyword.operator".to_owned()], "#333333".to_owned()),
        ],
    };
    assert_eq!(
        scope_color(&theme, "keyword.control.import"),
        parse_color("#222222")
    );
    assert_eq!(
        scope_color(&theme, "keyword.control"),
        parse_color("#111111")
    );
    assert_eq!(scope_color(&theme, "keywords"), None);
}

#[test]
fn builtin_dark_plus_parses_with_vscode_colors() {
    let theme = builtin_dark_plus().expect("bundled Dark+ theme parses");
    let palette = palette_from_theme(&theme);

    assert_eq!(palette.background, ColorU::new(0x1e, 0x1e, 0x1e, 255));
    assert_eq!(palette.gutter_added, parse_color("#2EA043"));
    assert_eq!(palette.gutter_modified, parse_color("#1B81A8"));
    assert_eq!(palette.gutter_deleted, parse_color("#F85149"));
    assert_eq!(palette.git_decorations.modified, parse_color("#E2C08D"));
    assert_eq!(palette.git_decorations.untracked, parse_color("#73C991"));
}

#[test]
fn builtin_dark_plus_colors_tokens_like_vscode() {
    let palette = palette_from_theme(&builtin_dark_plus().expect("bundled Dark+ theme parses"));
    let capture = |name: &str| palette.captures.get(name).copied();

    assert_eq!(capture("comment"), parse_color("#6A9955"));
    assert_eq!(capture("comment.documentation"), parse_color("#6A9955"));
    assert_eq!(capture("string"), parse_color("#CE9178"));
    assert_eq!(capture("keyword"), parse_color("#569CD6"));
    // Control flow and imports are purple in Dark+, other keywords blue.
    assert_eq!(capture("keyword.conditional"), parse_color("#C586C0"));
    assert_eq!(capture("keyword.import"), parse_color("#C586C0"));
    assert_eq!(capture("function"), parse_color("#DCDCAA"));
    assert_eq!(capture("type"), parse_color("#4EC9B0"));
    assert_eq!(capture("variable.parameter"), parse_color("#9CDCFE"));
    assert_eq!(capture("number"), parse_color("#B5CEA8"));
}
