use std::process::Command;

#[test]
fn config_help_is_available_without_starting_the_tui() {
    for args in [
        vec!["config"],
        vec!["config", "--help"],
        vec!["help", "config"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_bt-7274"))
            .args(args)
            .output()
            .unwrap();

        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("bt-7274 config"));
        assert!(help.contains("show"));
        assert!(help.contains("edit"));
        assert!(!help.contains('\u{1b}'));
    }
}

#[test]
fn invalid_config_subcommand_returns_a_usage_error() {
    let output = Command::new(env!("CARGO_BIN_EXE_bt-7274"))
        .args(["config", "unknown"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("unknown")
    );
}
