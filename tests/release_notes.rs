//! The release notes are made from CHANGELOG.md: on GitHub an `@name`
//! outside a code span mentions (and notifies) the GitHub user `name`, who
//! then shows up as a contributor of the release. Discord's `@everyone`,
//! `@here` and similar must be written in backticks.

fn unquoted_mentions(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for (n, line) in text.lines().enumerate() {
        // Drop the code spans: GitHub does not link mentions inside them.
        let mut plain = String::new();
        let mut in_code = false;
        for c in line.chars() {
            if c == '`' {
                in_code = !in_code;
            } else if !in_code {
                plain.push(c);
            }
        }
        let bytes = plain.as_bytes();
        for (i, b) in bytes.iter().enumerate() {
            let starts = *b == b'@'
                && bytes.get(i + 1).is_some_and(u8::is_ascii_alphanumeric)
                && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric());
            if starts {
                let name: String = plain[i + 1..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                    .collect();
                found.push(format!("line {}: @{name}", n + 1));
            }
        }
    }
    found
}

#[test]
fn changelog_mentions_nobody_on_github() {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/CHANGELOG.md")).unwrap();
    let found = unquoted_mentions(&text);
    assert!(
        found.is_empty(),
        "write these in backticks, or GitHub mentions a user: {found:?}"
    );
}

#[test]
fn finds_mentions_outside_code() {
    assert_eq!(
        unquoted_mentions("a @here/@everyone b"),
        ["line 1: @here", "line 1: @everyone"]
    );
    assert!(unquoted_mentions("`@here` and `@everyone`, me@example.com").is_empty());
}
