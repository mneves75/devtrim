//! Credential-pattern scan of coding-agent transcripts. Report only.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use support::{Allowed, Sandbox, Tree, json, run, write};

fn token() -> String {
    // Mixed letters and digits, long enough for every prefix this scan accepts.
    let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut body = String::new();
    while body.len() < 48 {
        body.push(char::from(alphabet[body.len() % alphabet.len()]));
    }
    format!("sk-ant-{body}")
}

#[test]
fn reports_a_mask_and_leaves_the_transcript_unchanged() {
    let sandbox = Sandbox::in_target("secrets-mask");
    let secret = token();
    let middle = &secret[12..28];
    let transcript = sandbox.path().join(".claude/projects/demo/session.jsonl");
    write(&transcript, &format!("please use {secret}\n"));
    let outside = sandbox.path().join("outside/secret.jsonl");
    write(
        &outside,
        "sk-ant-abcdefghijklmnopqrstuvwxyz0123456789abcd\n",
    );
    std::os::unix::fs::symlink(
        &outside,
        sandbox.path().join(".claude/projects/demo/linked.jsonl"),
    )
    .unwrap();
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["secrets", "--json"]);
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(document["operation"], "secrets");
    assert_eq!(document["applied"], false);
    let rendered = document.to_string();
    assert!(rendered.contains("Anthropic API key"), "{document}");
    assert!(rendered.contains("sk-ant-…"), "{document}");
    assert!(
        !rendered.contains(middle),
        "mask leaked key material: {document}"
    );
    assert!(
        !rendered.contains("abcdefghijklmnopqrstuvwxyz0123456789abcd"),
        "a symlink transcript must not be read: {document}"
    );
    assert_eq!(
        std::fs::read_to_string(&transcript).unwrap(),
        format!("please use {secret}\n")
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "secrets/unchanged",
    );
}

#[test]
fn mutation_flags_are_rejected() {
    let sandbox = Sandbox::in_target("secrets-flags");
    for flag in ["--apply", "--shred", "-y", "--yolo"] {
        let output = run(&sandbox, &["secrets", flag, "--json"]);
        let document = json(&output);
        assert_eq!(output.status.code(), Some(2), "{flag}: {document}");
        assert_eq!(document["operation"], "secrets");
        assert_eq!(document["applied"], false);
        assert!(
            document["errors"][0]
                .as_str()
                .unwrap()
                .contains("does not accept flag")
        );
    }
}
