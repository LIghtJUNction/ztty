extern crate alloc;

#[path = "../src/ssh_command.rs"]
mod ssh_command;

use ssh_command::{SshAction, SshOptions};

fn argv(options: &SshOptions, control_path: Option<&str>) -> Vec<String> {
    options
        .ssh_argv(control_path)
        .unwrap()
        .into_iter()
        .map(|value| value.into_string().unwrap())
        .collect()
}

#[test]
fn bare_alias_defaults_to_persistent_main_session() {
    let options = SshOptions::parse(&["build"]).unwrap();
    assert_eq!(options.destination, "build");
    assert_eq!(options.session, "main");
    assert_eq!(options.action, SshAction::Attach);
    assert_eq!(options.remote_bin, "ztty");
    let args = argv(&options, None);
    assert_eq!(
        &args[..6],
        &[
            "ssh",
            "-tt",
            "-o",
            "ServerAliveInterval=15",
            "-o",
            "ServerAliveCountMax=3"
        ]
    );
    assert_eq!(&args[6..8], &["--", "build"]);
    assert!(args.last().unwrap().ends_with("'mux' 'attach' 'main'"));
    assert!(!args.iter().any(|arg| arg.contains("StrictHostKeyChecking")));
}

#[test]
fn uri_user_port_and_ipv6_are_normalized_for_openssh() {
    for (input, destination, port) in [
        (
            "ssh://alice@build.example:2222",
            "alice@build.example",
            Some(2222),
        ),
        ("ssh://build", "build", None),
        ("ssh://[::1]:2200", "::1", Some(2200)),
        ("ssh://alice@[2001:db8::1234]", "alice@2001:db8::1234", None),
        ("alice@::1", "alice@::1", None),
        ("[::1]", "::1", None),
    ] {
        let options = SshOptions::parse(&[input]).unwrap();
        assert_eq!(options.destination, destination, "{input}");
        assert_eq!(options.port, port, "{input}");
    }
}

#[test]
fn connection_options_are_separate_literal_arguments() {
    let options = SshOptions::parse(&[
        "-F",
        "/tmp/isolated ssh config",
        "--port",
        "2200",
        "-i",
        "/tmp/id with 'quotes'",
        "--remote-bin",
        "/opt/ztty next/bin/ztty",
        "alice@build",
        "work",
    ])
    .unwrap();
    let args = argv(&options, Some("/tmp/private dir/control-%C"));
    for pair in [
        ["-F", "/tmp/isolated ssh config"],
        ["-p", "2200"],
        ["-i", "/tmp/id with 'quotes'"],
        ["-o", "ControlMaster=auto"],
        ["-o", "ControlPersist=60"],
        ["-S", "/tmp/private dir/control-%C"],
    ] {
        assert!(args.windows(2).any(|window| window == pair));
    }
    assert_eq!(
        args.last().unwrap(),
        "exec '/opt/ztty next/bin/ztty' 'mux' 'attach' 'work'"
    );
}

#[test]
fn list_has_no_implicit_session_and_requests_no_tty() {
    let options = SshOptions::parse(&["--list", "build", "--remote-bin", "ztty-next"]).unwrap();
    assert_eq!(options.action, SshAction::List);
    let args = argv(&options, None);
    assert_eq!(args[1], "-T");
    assert_eq!(args.last().unwrap(), "exec 'ztty-next' 'mux' 'list'");
    assert!(SshOptions::parse(&["--list", "build", "work"]).is_err());
}

#[test]
fn close_names_the_session_without_a_tty() {
    let options =
        SshOptions::parse(&["build", "work", "--close", "--remote-bin", "ztty-next"]).unwrap();
    assert_eq!(options.action, SshAction::Close);
    let args = argv(&options, None);
    assert_eq!(args[1], "-T");
    assert_eq!(
        args.last().unwrap(),
        "exec 'ztty-next' 'mux' 'close' 'work'"
    );
}

#[test]
fn attachment_control_and_layout_are_kept_separate() {
    for (flag, mode) in [
        ("--tab", "tab"),
        ("--split-right", "right"),
        ("--split-down", "down"),
    ] {
        let options =
            SshOptions::parse(&["build", "work", flag, "local-shell", "--observe"]).unwrap();
        assert_eq!(options.target, Some(("local-shell".into(), mode.into())));
        let args = argv(&options, None);
        assert!(
            args.last()
                .unwrap()
                .ends_with("'mux' 'attach' '--observe' 'work'")
        );
        assert!(!args.last().unwrap().contains("local-shell"));
    }
    let options =
        SshOptions::parse(&["build", "--terminal", "--takeover", "--", "sh", "-l"]).unwrap();
    assert!(options.terminal);
    assert!(
        argv(&options, None)
            .last()
            .unwrap()
            .ends_with("'mux' 'attach' '--takeover' 'main' '--' 'sh' '-l'")
    );
}

#[test]
fn unsafe_destinations_and_unsupported_uris_fail_before_spawn() {
    for input in [
        "-oProxyCommand=touch",
        "alice@-oProxyCommand=x",
        "build host",
        "build\nother",
        "build\r",
        "build\0",
        "build;touch",
        "$(touch)",
        "`touch`",
        "alice@@host",
        "ssh://",
        "ssh://host/path",
        "ssh://host/",
        "ssh://host?option=value",
        "ssh://host#fragment",
        "ssh://::1",
        "ssh://[not-an-ip]",
        "ssh://[::1]junk",
        "ssh://[::1",
        "ssh://user@[::1]:0",
        "host:2222",
    ] {
        assert!(SshOptions::parse(&[input]).is_err(), "accepted {input:?}");
    }
}

#[test]
fn invalid_values_and_conflicts_are_reported() {
    let cases: &[&[&str]] = &[
        &[],
        &["--", "build"],
        &["build", "--"],
        &["build", "--", ""],
        &["-p", "0", "build"],
        &["-p", "65536", "build"],
        &["-p", "+22", "build"],
        &["--port", "22", "ssh://host:23"],
        &["-p", "22", "-p", "23", "build"],
        &["--identity"],
        &["--identity", "", "build"],
        &["--identity", "x\ny", "build"],
        &["--ssh-config", "", "build"],
        &["-F", "one", "-F", "two", "build"],
        &["--remote-bin", "-option", "build"],
        &["--remote-bin", "", "build"],
        &["--list", "--close", "build"],
        &["--list", "build", "--observe"],
        &["--close", "build", "--", "sh"],
        &["--close", "build", "--terminal"],
        &["--observe", "--takeover", "build"],
        &["--observe", "build", "--", "sh"],
        &["--terminal", "--tab", "target", "build"],
        &["--tab", "one", "--split-right", "two", "build"],
        &["build", ".."],
        &["build", "bad/name"],
        &["build", "work", "sh"],
        &["build", "--", "printf", "line\nbreak"],
    ];
    for args in cases {
        assert!(SshOptions::parse(args).is_err(), "accepted {args:?}");
    }
}

#[test]
fn manually_constructed_options_are_validated_before_exec() {
    let original = SshOptions::parse(&["build"]).unwrap();
    let mut options = original.clone();
    options.destination = "-oProxyCommand=x".into();
    assert!(options.ssh_argv(None).is_err());
    options = original.clone();
    options.remote_bin = "-a".into();
    assert!(options.ssh_argv(None).is_err());
    options = original.clone();
    options.port = Some(0);
    assert!(options.ssh_argv(None).is_err());
    options = original.clone();
    options.command = vec!["sh".into(), "bad\0argument".into()];
    assert!(options.ssh_argv(None).is_err());
    assert!(original.ssh_argv(Some("/tmp/path\nInjected=yes")).is_err());
}

#[test]
fn input_and_quoted_command_sizes_are_bounded() {
    assert!(SshOptions::parse(&vec!["arg"; 257]).is_err());
    let oversized = "x".repeat(16 * 1024 + 1);
    assert!(SshOptions::parse(&["build", "--", "printf", &oversized]).is_err());
    let many_quotes = "'".repeat(16 * 1024);
    let options = SshOptions::parse(&["build", "--", "printf", &many_quotes]).unwrap();
    assert!(options.ssh_argv(None).is_err());
}

#[cfg(unix)]
mod shell_roundtrip {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "ztty-ssh-command-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::create_dir(path.join("bin")).unwrap();
            symlink("/bin/sh", path.join("bin/sh")).unwrap();
            Self(path)
        }

        fn path(&self) -> String {
            self.0.join("bin").to_string_lossy().into_owned()
        }

        fn executable(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "#!/bin/sh\nprintf '%s\\000' \"$@\"\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            path
        }

        fn run(&self, options: &SshOptions, path: &str) -> Vec<String> {
            let args = argv(options, None);
            let output = Command::new("/bin/sh")
                .args(["-c", args.last().unwrap()])
                .env_clear()
                .env("HOME", &self.0)
                .env("PATH", path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
                .stdout
                .strip_suffix(&[0])
                .unwrap()
                .split(|byte| *byte == 0)
                .map(|value| String::from_utf8(value.to_vec()).unwrap())
                .collect()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn quotes_and_shell_operators_reach_the_remote_program_literally() {
        let fixture = Fixture::new();
        let binary = fixture.executable("remote bin's ztty");
        let tokens = [
            "printf",
            "",
            "two words",
            "it's \"quoted\"",
            "$(printf injected)",
            "`printf injected`",
            "a; echo injected",
            "back\\slash",
            "$HOME",
            "*?[abc]",
            "汉字",
        ];
        let mut args = vec![
            "--remote-bin",
            binary.to_str().unwrap(),
            "build",
            "work",
            "--",
        ];
        args.extend(tokens);
        let options = SshOptions::parse(&args).unwrap();
        let actual = fixture.run(&options, &fixture.path());
        let mut expected = vec!["mux", "attach", "work", "--"];
        expected.extend(tokens);
        assert_eq!(actual, expected);
    }

    #[test]
    fn default_remote_binary_falls_back_to_the_users_local_bin() {
        let fixture = Fixture::new();
        fixture.executable(".local/bin/ztty");
        let options =
            SshOptions::parse(&["build", "work", "--", "printf", "literal '$HOME'"]).unwrap();
        assert_eq!(
            fixture.run(&options, &fixture.path()),
            ["mux", "attach", "work", "--", "printf", "literal '$HOME'"]
        );
    }

    #[test]
    fn default_remote_binary_prefers_the_remote_path() {
        let fixture = Fixture::new();
        fixture.executable("bin/ztty");
        let options = SshOptions::parse(&["build", "--list"]).unwrap();
        assert_eq!(fixture.run(&options, &fixture.path()), ["mux", "list"]);
    }
}
