/// POSIX field splitting of an EXPANDED field (unquoted-origin only) using IFS.
pub fn fields(input: &str, ifs: &str) -> Vec<String> {
    if ifs.is_empty() {
        return vec![input.to_string()];
    }

    let ws: Vec<char> = ifs
        .chars()
        .filter(|c| *c == ' ' || *c == '\t' || *c == '\n')
        .collect();
    let delim: Vec<char> = ifs
        .chars()
        .filter(|c| *c != ' ' && *c != '\t' && *c != '\n')
        .collect();

    let is_ws = |c: char| ws.contains(&c);
    let is_delim = |c: char| delim.contains(&c);

    let chars: Vec<char> = input.chars().collect();
    let n = chars.len();

    let mut i = 0usize;
    while i < n && is_ws(chars[i]) {
        i += 1;
    }

    let mut fields: Vec<String> = Vec::new();
    let mut current = String::new();

    while i < n {
        let c = chars[i];
        if is_delim(c) {
            fields.push(std::mem::take(&mut current));
            i += 1;
            while i < n && is_ws(chars[i]) {
                i += 1;
            }
        } else if is_ws(c) {
            while i < n && is_ws(chars[i]) {
                i += 1;
            }
            if i >= n {
                if !current.is_empty() || !fields.is_empty() {
                    fields.push(std::mem::take(&mut current));
                }
                break;
            }
            if is_delim(chars[i]) {
                continue;
            }
            fields.push(std::mem::take(&mut current));
        } else {
            current.push(c);
            i += 1;
        }
    }

    if !current.is_empty() {
        fields.push(current);
    } else if !fields.is_empty() && n > 0 && is_delim(chars[n - 1]) {
        fields.push(String::new());
    } else if fields.is_empty() && !current.is_empty() {
        fields.push(current);
    }

    while !fields.is_empty() && fields[0].is_empty() {
        fields.remove(0);
    }
    while !fields.is_empty() && fields[fields.len() - 1].is_empty() {
        fields.pop();
    }

    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ifs_basic() {
        assert_eq!(fields("a b c", " \t\n"), vec!["a", "b", "c"]);
        assert_eq!(fields("  a   b  ", " \t\n"), vec!["a", "b"]);
        assert_eq!(fields("", " \t\n"), Vec::<String>::new());
        assert_eq!(fields("   ", " \t\n"), Vec::<String>::new());
    }

    #[test]
    fn empty_ifs() {
        assert_eq!(fields("a b", ""), vec!["a b"]);
    }

    #[test]
    fn colon() {
        assert_eq!(fields("a:b:c", ":"), vec!["a", "b", "c"]);
        assert_eq!(fields("a::b", ":"), vec!["a", "", "b"]);
        assert_eq!(fields("::", ":"), Vec::<String>::new());
        assert_eq!(fields(":a", ":"), vec!["a"]);
        assert_eq!(fields("a:", ":"), vec!["a"]);
    }

    #[test]
    fn colon_whitespace_adjacent() {
        assert_eq!(fields("a: b", ": "), vec!["a", "b"]);
        assert_eq!(fields("a :b", ": "), vec!["a", "b"]);
        assert_eq!(fields("a : b", ": "), vec!["a", "b"]);
    }

    #[test]
    fn mixed() {
        assert_eq!(fields("a,b c", ", "), vec!["a", "b", "c"]);
        assert_eq!(fields("a,,b", ","), vec!["a", "", "b"]);
    }
}
