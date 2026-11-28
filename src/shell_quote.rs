//! POSIX single-quote escaping for shell command strings.

use verus_builtin_macros::{verus, verus_spec, verus_verify};
// Used only by the `verus!` block below; erased builds see no vstd items.
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// The shell-quoted form of `s[0..i]`: every char passes through, except each
/// `'`, which becomes the four-char escape sequence `'\''`.
pub open spec fn quoted_prefix(s: Seq<char>, i: int) -> Seq<char>
    decreases i,
{
    if i <= 0 {
        Seq::empty()
    } else if s[i - 1] == '\'' {
        quoted_prefix(s, i - 1).add(Seq::empty().push('\'').push('\\').push('\'').push('\''))
    } else {
        quoted_prefix(s, i - 1).add(Seq::empty().push(s[i - 1]))
    }
}

/// Build the quoted form of `chars` with the `'\''` escape for each `'`.
///
/// Fully verified against [`quoted_prefix`]: the result is exactly the spec
/// model applied to the whole input.
fn quote_chars(chars: &[char]) -> (result: Vec<char>)
    ensures
        result@ == quoted_prefix(chars@, chars@.len() as int),
{
    let mut result: Vec<char> = Vec::new();
    let mut i: usize = 0;
    while i < chars.len()
        invariant
            i <= chars.len(),
            result@ == quoted_prefix(chars@, i as int),
        decreases chars.len() - i,
    {
        let c = chars[i];
        if c == '\'' {
            result.push('\'');
            result.push('\\');
            result.push('\'');
            result.push('\'');
        } else {
            result.push(c);
        }
        i += 1;
    }
    result
}

} // verus!

/// Quote `value` for a POSIX shell: wrap it in single quotes and replace each
/// `'` inside with the escape sequence `'\''`, so a shell parses the quoted
/// string back to `value`.
///
/// The conversion logic is verified in [`quote_chars`]; this wrapper is
/// trusted only for the standard-library conversions `str::chars` and
/// `String: FromIterator<char>` (vstd has no specifications for them).
#[verus_verify(external_body)]
#[verus_spec(result =>
    ensures
        result@ == Seq::empty().push('\'')
            .add(quoted_prefix(value@, value@.len() as int))
            .push('\''),
)]
pub(crate) fn shell_quote(value: &str) -> String {
    let mut result = String::new();
    result.push('\'');
    let chars: Vec<char> = value.chars().collect();
    let quoted = quote_chars(&chars);
    result.extend(quoted);
    result.push('\'');
    result
}

#[cfg(test)]
mod tests {
    use super::shell_quote;

    #[test]
    fn should_quote_plain_string() {
        assert_eq!(shell_quote("abc"), "'abc'");
    }

    #[test]
    fn should_quote_empty_string() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn should_escape_embedded_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn should_preserve_non_ascii_chars() {
        assert_eq!(shell_quote("café"), "'café'");
    }
}
