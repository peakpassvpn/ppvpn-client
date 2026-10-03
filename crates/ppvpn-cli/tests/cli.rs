//! Golden tests for the command-line contract: text and `--json` output,
//! where each goes, and exit codes. Every invocation runs with its own
//! temporary home, so tests run in parallel and never touch the real one.

use ppvpn_cli::buildinfo;
use ppvpn_cli::env::{Env, Os};
use serde_json::Value;

struct Outcome {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Outcome {
    fn json(&self) -> Value {
        assert_eq!(
            self.stdout.lines().count(),
            1,
            "--json must print exactly one line: {:?}",
            self.stdout
        );
        serde_json::from_str(&self.stdout).expect("stdout is one JSON value")
    }
}

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn env(&self) -> Env {
        let home = self.dir.path().to_str().unwrap();
        let config = format!("{home}/config");
        let state = format!("{home}/state");
        Env::with_vars(
            Os::current(),
            &[
                ("HOME", home),
                ("XDG_CONFIG_HOME", &config),
                ("XDG_STATE_HOME", &state),
            ],
        )
    }

    fn run(&self, args: &[&str]) -> Outcome {
        let argv: Vec<std::ffi::OsString> = std::iter::once("ppvpn")
            .chain(args.iter().copied())
            .map(Into::into)
            .collect();
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let code = ppvpn_cli::run(&argv, &self.env(), &mut stdout, &mut stderr);
        Outcome {
            code,
            stdout: String::from_utf8(stdout).unwrap(),
            stderr: String::from_utf8(stderr).unwrap(),
        }
    }
}

fn run(args: &[&str]) -> Outcome {
    Home::new().run(args)
}

#[test]
fn version_text_and_json() {
    let out = run(&["version"]);
    assert_eq!(
        (out.code, out.stdout.as_str(), out.stderr.as_str()),
        (0, format!("ppvpn {}\n", buildinfo::version()).as_str(), "")
    );
    let out = run(&["--json", "version"]);
    assert_eq!(out.code, 0);
    assert_eq!(
        out.json(),
        serde_json::json!({"ok": true, "cli_version": buildinfo::version()})
    );
    // The global flag may follow the subcommand too.
    assert_eq!(run(&["version", "--json"]).json()["ok"], true);
}

#[test]
fn errors_go_to_stderr_in_text_mode_and_stdout_in_json_mode() {
    let out = run(&["--bogus"]);
    assert_eq!(out.code, 2);
    assert!(out.stdout.is_empty(), "{:?}", out.stdout);
    assert!(out.stderr.starts_with("Error: "), "{:?}", out.stderr);

    let out = run(&["--json", "mode", "direct"]);
    assert_eq!(out.code, 2);
    assert!(out.stderr.is_empty(), "{:?}", out.stderr);
    assert_eq!(
        out.json(),
        serde_json::json!({"ok": false, "code": "INVALID_ARGUMENT", "message": "mode must be rules or global", "retryable": false})
    );
}

#[test]
fn unknown_commands_and_missing_subcommands_are_argument_errors() {
    for args in [&["frobnicate"][..], &[], &["--json", "frobnicate"]] {
        let out = run(args);
        assert_eq!(out.code, 2, "{args:?}");
        if args.first() == Some(&"--json") {
            assert_eq!(out.json()["code"], "INVALID_ARGUMENT");
        }
    }
}

#[test]
fn help_exits_zero_on_stdout() {
    let out = run(&["--help"]);
    assert_eq!(out.code, 0);
    assert!(
        out.stdout.contains("PeakPass VPN terminal client"),
        "{}",
        out.stdout
    );
}

#[test]
fn arguments_are_checked_before_anything_else() {
    let cases: &[&[&str]] = &[
        &["use"],
        &["use", "a", "b"],
        &["use", " "],
        &["proxy", "credential", "a", "b"],
        &["proxy", "credential", " "],
        &["probe"],
        &["probe", "a", "b"],
        &["probe", "node", "--all"],
        &["probe", "--all", "--type", "availability"],
        &["probe", "node", "--type", "ping"],
        &["probe", "--all", "--timeout", "3m"],
        &["probe", "--all", "--timeout", "0s"],
        &["probe", "--all", "--timeout", "soon"],
        &["probe", "--all", "--concurrency", "0"],
        &["probe", "--all", "--concurrency", "33"],
        &["mode", "rules", "global"],
        &["ingress", "a", "b"],
        &["ingress", "pin", "node"],
        &["ingress", "pin", " ", "9001"],
        &["ingress", "auto"],
        &["completion"],
        &["completion", "powershell"],
        &["--json", "completion", "bash"],
        &["status", "extra"],
        &["start", "--background"],
    ];
    for args in cases {
        let out = run(args);
        assert_eq!(out.code, 2, "{args:?}: {} {}", out.stdout, out.stderr);
        if args.first() != Some(&"--json") {
            assert!(
                out.stdout.is_empty(),
                "{args:?} wrote to stdout: {:?}",
                out.stdout
            );
        }
    }
}

#[test]
fn completion_scripts_for_each_shell() {
    for shell in ["bash", "zsh", "fish"] {
        let out = run(&["completion", shell]);
        assert_eq!(out.code, 0, "{shell}: {}", out.stderr);
        assert!(out.stdout.contains("ppvpn"), "{shell}");
        assert!(
            out.stdout.contains("ingress"),
            "{shell}: subcommands complete"
        );
    }
}

#[test]
fn mode_defaults_to_rules_and_persists() {
    let home = Home::new();
    let out = home.run(&["mode"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (0, "Routing mode: rules\n")
    );
    assert_eq!(
        home.run(&["--json", "mode"]).json(),
        serde_json::json!({"ok": true, "routing_mode": "rules"})
    );

    let out = home.run(&["--json", "mode", "global"]);
    assert_eq!(
        out.json(),
        serde_json::json!({"ok": true, "routing_mode": "global", "applied": false})
    );
    assert_eq!(home.run(&["mode"]).stdout, "Routing mode: global\n");

    let settings = ppvpn_cli::paths::Paths::resolve(&home.env())
        .unwrap()
        .settings;
    assert!(settings.starts_with(home.dir.path()));
    let text = std::fs::read_to_string(&settings).unwrap();
    assert!(text.contains("\"global\""), "{text}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&settings).unwrap().permissions().mode() & 0o777,
        0o600
    );

    assert_eq!(home.run(&["mode", "rules"]).stdout, "Routing mode: rules\n");
}

#[test]
fn doctor_reports_build_and_directories() {
    let home = Home::new();
    let out = home.run(&["--json", "doctor"]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let value = out.json();
    assert_eq!(value["ok"], true);
    assert_eq!(value["cli_version"], buildinfo::version());
    for field in [
        "platform",
        "build_profile",
        "api_origin",
        "runtime_directory",
        "state_directory",
        "settings_file",
    ] {
        assert!(value[field].is_string(), "{field}: {value}");
    }
    let base = home.dir.path().to_str().unwrap();
    for field in ["runtime_directory", "state_directory", "settings_file"] {
        assert!(
            value[field].as_str().unwrap().starts_with(base),
            "{field} outside the test home: {value}"
        );
    }
    let out = home.run(&["doctor"]);
    assert!(out.stdout.starts_with("Platform: "), "{}", out.stdout);
}

#[test]
fn commands_not_wired_to_the_daemon_yet() {
    for args in [
        &["use", "hk-1"][..],
        &["proxy", "credential"],
        &["nodes"],
        &["traffic"],
    ] {
        let out = run(&[&["--json"][..], args].concat());
        assert_eq!(out.code, 1, "{args:?}");
        assert_eq!(out.json()["code"], "NOT_IMPLEMENTED", "{args:?}");
    }
}
