pub fn normalize_jsonc(content: &[u8]) -> Vec<u8> {
    let mut output = content.to_vec();
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while index < output.len() {
        let byte = output[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            index += 1;
            continue;
        }
        if byte == b'/' && output.get(index + 1) == Some(&b'/') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            while index < output.len() && !matches!(output[index], b'\r' | b'\n') {
                output[index] = b' ';
                index += 1;
            }
            continue;
        }
        if byte == b'/' && output.get(index + 1) == Some(&b'*') {
            output[index] = b' ';
            output[index + 1] = b' ';
            index += 2;
            while index + 1 < output.len() {
                if output[index] == b'*' && output[index + 1] == b'/' {
                    output[index] = b' ';
                    output[index + 1] = b' ';
                    index += 2;
                    break;
                }
                if !matches!(output[index], b'\r' | b'\n') {
                    output[index] = b' ';
                }
                index += 1;
            }
            continue;
        }
        index += 1;
    }

    index = 0;
    in_string = false;
    escaped = false;
    while index < output.len() {
        let byte = output[index];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else if byte == b'"' {
            in_string = true;
        } else if byte == b',' {
            let mut next = index + 1;
            while output.get(next).is_some_and(u8::is_ascii_whitespace) {
                next += 1;
            }
            if output
                .get(next)
                .is_some_and(|byte| matches!(byte, b'}' | b']'))
            {
                output[index] = b' ';
            }
        }
        index += 1;
    }
    output
}

#[cfg(test)]
mod tests {
    use super::normalize_jsonc;

    #[test]
    fn preserves_comment_markers_inside_strings() {
        let normalized = normalize_jsonc(
            br#"{"url":"https://example.test/a/*literal*/",// comment
                "values":[1,],}"#,
        );
        let parsed: serde_json::Value = serde_json::from_slice(&normalized).expect("jsonc");
        assert_eq!(parsed["url"], "https://example.test/a/*literal*/");
        assert_eq!(parsed["values"], serde_json::json!([1]));
    }
}
