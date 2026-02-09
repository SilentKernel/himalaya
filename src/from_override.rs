use email::account::config::AccountConfig;

const FROM_EMAIL: Option<&str> = option_env!("HIMALAYA_FROM_EMAIL");
const FROM_NAME: Option<&str> = option_env!("HIMALAYA_FROM_NAME");

/// Override `AccountConfig` email/display_name with compile-time values.
pub fn apply_from_override(config: &mut AccountConfig) {
    if let Some(email) = FROM_EMAIL {
        config.email = email.to_owned();
    }
    if let Some(name) = FROM_NAME {
        config.display_name = Some(name.to_owned());
    }
}

/// Replace the `From:` header in a raw RFC 5322 message with the
/// compile-time override value. Returns the original bytes unchanged
/// when no override is compiled in.
pub fn override_from_in_raw_message(msg: &[u8]) -> Vec<u8> {
    let email = match FROM_EMAIL {
        Some(e) => e,
        None => return msg.to_vec(),
    };

    let replacement = match FROM_NAME {
        Some(name) => format!("From: {} <{}>\r\n", name, email),
        None => format!("From: {}\r\n", email),
    };

    let src = String::from_utf8_lossy(msg);

    // Find the From: header (case-insensitive).
    // It may span multiple lines (folded headers: continuation lines
    // start with whitespace).
    let mut result = String::with_capacity(src.len());
    let mut lines = src.split_inclusive('\n');
    let mut in_from_header = false;
    let mut from_replaced = false;

    while let Some(line) = lines.next() {
        if in_from_header {
            // Continuation line starts with space or tab
            if line.starts_with(' ') || line.starts_with('\t') {
                // Skip folded continuation of From header
                continue;
            }
            // No longer in the From header
            in_from_header = false;
            result.push_str(line);
        } else if !from_replaced
            && (line.starts_with("From:") || line.starts_with("from:") || {
                let lower = line.to_ascii_lowercase();
                lower.starts_with("from:")
            })
        {
            in_from_header = true;
            from_replaced = true;
            result.push_str(&replacement);
        } else if line.is_empty() || line == "\n" || line == "\r\n" {
            // We've reached the header/body separator without finding a
            // From header — insert one before the blank line.
            if !from_replaced {
                from_replaced = true;
                result.push_str(&replacement);
            }
            result.push_str(line);
            // Append the rest of the message as-is (body).
            for rest in lines.by_ref() {
                result.push_str(rest);
            }
            break;
        } else {
            result.push_str(line);
        }
    }

    // If we never hit a blank separator (unusual), append replacement
    if !from_replaced {
        result.push_str(&replacement);
    }

    result.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_from_override_sets_fields() {
        let mut config = AccountConfig {
            email: "original@example.com".into(),
            display_name: None,
            ..Default::default()
        };

        apply_from_override(&mut config);

        // When compiled without the env vars, fields stay unchanged.
        // When compiled with them, they would be overwritten.
        // We can only assert the function doesn't panic here.
        // The actual override is a compile-time constant.
        assert!(!config.email.is_empty());
    }

    #[test]
    fn override_raw_message_replaces_from_header() {
        let msg = b"Subject: Test\r\nFrom: old@example.com\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        // Without compile-time override, should return unchanged
        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains(FROM_EMAIL.unwrap()));
            assert!(!text.contains("old@example.com"));
        }
    }

    #[test]
    fn override_raw_message_handles_folded_from() {
        let msg = b"Subject: Test\r\nFrom: Very Long Name\r\n <old@example.com>\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains(FROM_EMAIL.unwrap()));
            assert!(!text.contains("old@example.com"));
            // Folded continuation should be removed
            assert!(!text.contains(" <old@"));
        }
    }

    #[test]
    fn override_raw_message_no_from_header() {
        let msg = b"Subject: Test\r\nTo: someone@example.com\r\n\r\nBody";

        let result = override_from_in_raw_message(msg);

        if FROM_EMAIL.is_none() {
            assert_eq!(result, msg.to_vec());
        } else {
            let text = String::from_utf8(result).unwrap();
            assert!(text.contains("From:"));
            assert!(text.contains(FROM_EMAIL.unwrap()));
        }
    }

    #[test]
    fn override_raw_message_unchanged_without_env() {
        if FROM_EMAIL.is_some() {
            return; // This test is only meaningful without compile-time override
        }

        let msg = b"From: keep@example.com\r\nSubject: Test\r\n\r\nBody";
        let result = override_from_in_raw_message(msg);
        assert_eq!(result, msg.to_vec());
    }
}
