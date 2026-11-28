//! POSIX single-quote escaping for shell command strings.

/// Quote `value` for a POSIX shell: wrap it in single quotes and replace each
/// `'` inside with the escape sequence `'\''`, so a shell parses the quoted
/// string back to `value`.
pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}
