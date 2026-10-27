//! The safety gate: OFAST's capture works only in a CS2 started with `-insecure`, which keeps it off every
//! VAC-secured server (decision 022). The launcher checks the running `cs2.exe`'s command line before attaching, and
//! the DLL checks its own process's again when it loads (refusing to load without it).

/// Whether a Windows command line has `-insecure` as an argument of its own, any case: the arguments split as
/// Windows programs split theirs ([`arguments`]), so a quoted path that contains the word, or the word glued to
/// another, is no.
pub fn is_insecure(command_line: &str) -> bool {
    arguments(command_line)
        .iter()
        .skip(1)
        .any(|a| a.eq_ignore_ascii_case("-insecure"))
}

/// A command line split into its arguments by the rules of Windows' `CommandLineToArgvW` and the C runtime: the
/// program name first (up to the next space, or between quotes, backslashes as they are); then arguments separated
/// by spaces and tabs outside double quotes, where `2n` backslashes before a quote are `n` backslashes and the quote
/// opens or closes a quoted stretch, `2n + 1` backslashes before a quote are `n` backslashes and a quote, and other
/// backslashes are themselves.
pub fn arguments(command_line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = command_line.chars().collect();
    let mut i = 0;
    // The program name.
    let mut name = String::new();
    if chars.first() == Some(&'"') {
        i = 1;
        while i < chars.len() && chars[i] != '"' {
            name.push(chars[i]);
            i += 1;
        }
        i += 1;
    } else {
        while i < chars.len() && chars[i] != ' ' && chars[i] != '\t' {
            name.push(chars[i]);
            i += 1;
        }
    }
    out.push(name);
    // The arguments.
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                let run = chars[i..].iter().take_while(|&&c| c == '\\').count();
                i += run;
                if chars.get(i) == Some(&'"') {
                    current.extend(std::iter::repeat_n('\\', run / 2));
                    if run % 2 == 1 {
                        current.push('"');
                        i += 1;
                    }
                } else {
                    current.extend(std::iter::repeat_n('\\', run));
                }
                started = true;
                continue;
            }
            '"' => {
                quoted = !quoted;
                started = true;
            }
            ' ' | '\t' if !quoted => {
                if started {
                    out.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
        i += 1;
    }
    if started {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insecure_must_be_an_argument_of_its_own() {
        let steam = r#""C:\Program Files (x86)\Steam\steamapps\common\Counter-Strike Global Offensive\game\bin\win64\cs2.exe" -steam -insecure -novid -console"#;
        assert!(is_insecure(steam));
        assert!(is_insecure("cs2.exe -INSECURE"));
        assert!(!is_insecure(r#""C:\games\cs2.exe" -steam -novid"#));
        assert!(!is_insecure("cs2.exe -insecurex"));
        assert!(!is_insecure("cs2.exe +insecure"));
        assert!(!is_insecure(r#""C:\my -insecure folder\cs2.exe" -steam"#));
        assert!(!is_insecure(""));
        // The program name is not an argument.
        assert!(!is_insecure("-insecure"));
    }

    #[test]
    fn escaped_quotes_do_not_end_a_quoted_argument() {
        // `"a\" -insecure"` is one argument, `a" -insecure`, to Windows.
        assert!(!is_insecure(r#"cs2.exe "a\" -insecure""#));
        assert_eq!(
            arguments(r#"cs2.exe "a\" -insecure""#),
            vec!["cs2.exe", r#"a" -insecure"#]
        );
    }

    #[test]
    fn arguments_split_as_windows_does() {
        assert_eq!(
            arguments(r#"prog "a b" c  "d" e"f" g\\"h i" j\k"#),
            vec!["prog", "a b", "c", "d", "ef", r"g\h i", r"j\k"]
        );
        assert_eq!(
            arguments(r#""C:\a b\x.exe" -y"#),
            vec![r"C:\a b\x.exe", "-y"]
        );
    }
}
