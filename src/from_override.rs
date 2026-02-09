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

const CC_EMAIL: &str = "ludo@hey.com";

/// Ensure the `Cc:` header in a raw RFC 5322 message contains
/// `CC_EMAIL`. If the header is missing, one is inserted. If it
/// already lists `CC_EMAIL`, the message is returned unchanged.
pub fn inject_cc_in_raw_message(msg: &[u8]) -> Vec<u8> {
    let src = String::from_utf8_lossy(msg);

    // Collect the full Cc and To header values (may be folded across lines).
    let mut cc_value = String::new();
    let mut to_value = String::new();
    let mut cc_start: Option<usize> = None;
    let mut cc_end: usize = 0;
    let mut pos: usize = 0;
    let mut in_cc = false;
    let mut in_to = false;
    let mut header_end: Option<usize> = None;

    for line in src.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();

        if in_cc {
            if line.starts_with(' ') || line.starts_with('\t') {
                // Folded continuation
                cc_value.push_str(line.trim_end());
                cc_end = pos;
                continue;
            }
            in_cc = false;
        }

        if in_to {
            if line.starts_with(' ') || line.starts_with('\t') {
                to_value.push_str(line.trim_end());
                continue;
            }
            in_to = false;
        }

        let lower = line.to_ascii_lowercase();

        if cc_start.is_none() && lower.starts_with("cc:") {
            in_cc = true;
            cc_start = Some(line_start);
            cc_value = line["Cc:".len()..].trim_end().to_string();
            cc_end = pos;
            continue;
        }

        if lower.starts_with("to:") {
            in_to = true;
            to_value = line["To:".len()..].trim_end().to_string();
            continue;
        }

        // Blank line = header/body separator
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            header_end = Some(line_start);
            break;
        }
    }

    // Already present in Cc or To?
    if cc_value.to_ascii_lowercase().contains(CC_EMAIL)
        || to_value.to_ascii_lowercase().contains(CC_EMAIL)
    {
        return msg.to_vec();
    }

    let mut result = String::with_capacity(src.len() + CC_EMAIL.len() + 10);

    if let Some(start) = cc_start {
        // Existing Cc header — append our address
        result.push_str(&src[..start]);
        let old_value = src[start..cc_end].trim_end_matches(|c| c == '\r' || c == '\n');
        result.push_str(old_value);
        result.push_str(", ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
        result.push_str(&src[cc_end..]);
    } else if let Some(sep) = header_end {
        // No Cc header — insert one before the blank line
        result.push_str(&src[..sep]);
        result.push_str("Cc: ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
        result.push_str(&src[sep..]);
    } else {
        // No blank separator found (unusual) — append at end
        result.push_str(&src);
        result.push_str("Cc: ");
        result.push_str(CC_EMAIL);
        result.push_str("\r\n");
    }

    result.into_bytes()
}

/// Ensure the `Cc:` header in a template string contains `CC_EMAIL`.
/// Templates use `\n` line endings and the same `Header: value` format.
pub fn inject_cc_in_tpl(content: &mut String) {
    // Find existing Cc and To headers in template text (plain \n line endings)
    let mut cc_value = String::new();
    let mut to_value = String::new();
    let mut cc_start: Option<usize> = None;
    let mut cc_end: usize = 0;
    let mut pos: usize = 0;
    let mut in_cc = false;
    let mut in_to = false;
    let mut header_end: Option<usize> = None;

    for line in content.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();

        if in_cc {
            if line.starts_with(' ') || line.starts_with('\t') {
                cc_value.push_str(line.trim_end());
                cc_end = pos;
                continue;
            }
            in_cc = false;
        }

        if in_to {
            if line.starts_with(' ') || line.starts_with('\t') {
                to_value.push_str(line.trim_end());
                continue;
            }
            in_to = false;
        }

        let lower = line.to_ascii_lowercase();

        if cc_start.is_none() && lower.starts_with("cc:") {
            in_cc = true;
            cc_start = Some(line_start);
            cc_value = line["Cc:".len()..].trim_end().to_string();
            cc_end = pos;
            continue;
        }

        if lower.starts_with("to:") {
            in_to = true;
            to_value = line["To:".len()..].trim_end().to_string();
            continue;
        }

        let trimmed = line.trim_end_matches(|c: char| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            header_end = Some(line_start);
            break;
        }
    }

    if cc_value.to_ascii_lowercase().contains(CC_EMAIL)
        || to_value.to_ascii_lowercase().contains(CC_EMAIL)
    {
        return;
    }

    if let Some(start) = cc_start {
        let old = content[start..cc_end]
            .trim_end_matches(|c: char| c == '\r' || c == '\n')
            .to_string();
        let replacement = format!("{}, {}\n", old, CC_EMAIL);
        content.replace_range(start..cc_end, &replacement);
    } else if let Some(sep) = header_end {
        let insertion = format!("Cc: {}\n", CC_EMAIL);
        content.insert_str(sep, &insertion);
    } else {
        content.push_str(&format!("Cc: {}\n", CC_EMAIL));
    }
}

/// Always inject fresh `Date:`, `Message-ID:`, and `MIME-Version:` headers
/// into a raw RFC 5322 message. If any of these headers already exist, they
/// are removed and replaced with fresh values. New headers are inserted
/// just before the header/body separator (the first blank line).
pub fn inject_missing_headers(msg: &[u8]) -> Vec<u8> {
    use chrono::Utc;
    use uuid::Uuid;

    let src = String::from_utf8_lossy(msg);

    // Names of headers we want to replace (lowercase for comparison).
    const TARGETS: &[&str] = &["date:", "message-id:", "mime-version:"];

    // Collect non-target header lines + track where the body starts.
    let mut header_lines: Vec<&str> = Vec::new();
    let mut lines = src.split_inclusive('\n');
    let mut body_start: Option<usize> = None;
    let mut pos: usize = 0;
    let mut skip_folded = false;

    for line in lines.by_ref() {
        let line_start = pos;
        pos += line.len();

        // Check for header/body separator (blank line).
        let trimmed = line.trim_end_matches(|c| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            body_start = Some(line_start);
            break;
        }

        if skip_folded {
            if line.starts_with(' ') || line.starts_with('\t') {
                // Folded continuation of a target header — skip it.
                continue;
            }
            skip_folded = false;
        }

        let lower = line.to_ascii_lowercase();
        if TARGETS.iter().any(|t| lower.starts_with(t)) {
            skip_folded = true;
            continue;
        }

        header_lines.push(line);
    }

    // Build replacement headers.
    let date = Utc::now().format("%a, %d %b %Y %H:%M:%S %z");
    let msg_id = format!("<{}@localhost>", Uuid::new_v4());

    let mut result = String::with_capacity(src.len() + 128);

    for line in &header_lines {
        result.push_str(line);
    }

    result.push_str(&format!("Date: {}\r\n", date));
    result.push_str(&format!("Message-ID: {}\r\n", msg_id));
    result.push_str("MIME-Version: 1.0\r\n");

    // Re-append the blank separator and body.
    if let Some(sep) = body_start {
        result.push_str(&src[sep..]);
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

    // --- inject_cc_in_raw_message tests ---

    #[test]
    fn inject_cc_raw_no_cc_header() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains(&format!("Cc: {}", CC_EMAIL)));
        assert!(text.contains("\r\n\r\nBody"));
    }

    #[test]
    fn inject_cc_raw_existing_cc_without_target() {
        let msg = b"From: a@example.com\r\nCc: other@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains("other@example.com"));
        assert!(text.contains(CC_EMAIL));
    }

    #[test]
    fn inject_cc_raw_already_present() {
        let msg = format!(
            "From: a@example.com\r\nCc: {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, msg.as_bytes().to_vec());
    }

    #[test]
    fn inject_cc_raw_folded_header() {
        let msg = b"From: a@example.com\r\nCc: first@example.com,\r\n second@example.com\r\nSubject: hi\r\n\r\nBody";
        let result = inject_cc_in_raw_message(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains(CC_EMAIL));
        assert!(text.contains("first@example.com"));
    }

    // --- inject_cc_in_tpl tests ---

    #[test]
    fn inject_cc_tpl_no_cc_header() {
        let mut tpl = "From: a@example.com\nSubject: hi\n\nBody".to_string();
        inject_cc_in_tpl(&mut tpl);
        assert!(tpl.contains(&format!("Cc: {}", CC_EMAIL)));
        assert!(tpl.contains("\n\nBody"));
    }

    #[test]
    fn inject_cc_tpl_existing_cc_without_target() {
        let mut tpl = "From: a@example.com\nCc: other@example.com\nSubject: hi\n\nBody".to_string();
        inject_cc_in_tpl(&mut tpl);
        assert!(tpl.contains("other@example.com"));
        assert!(tpl.contains(CC_EMAIL));
    }

    #[test]
    fn inject_cc_tpl_already_present() {
        let original = format!(
            "From: a@example.com\nCc: {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    #[test]
    fn inject_cc_tpl_already_present_among_others() {
        let original = format!(
            "From: a@example.com\nCc: other@example.com, {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    // --- inject_cc skips when CC_EMAIL is in To: ---

    #[test]
    fn inject_cc_raw_skips_when_in_to() {
        let msg = format!(
            "From: a@example.com\r\nTo: {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let original = msg.as_bytes().to_vec();
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, original);
    }

    #[test]
    fn inject_cc_raw_skips_when_in_to_among_others() {
        let msg = format!(
            "From: a@example.com\r\nTo: other@x.com, {}\r\nSubject: hi\r\n\r\nBody",
            CC_EMAIL
        );
        let original = msg.as_bytes().to_vec();
        let result = inject_cc_in_raw_message(msg.as_bytes());
        assert_eq!(result, original);
    }

    #[test]
    fn inject_cc_tpl_skips_when_in_to() {
        let original = format!(
            "From: a@example.com\nTo: {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    #[test]
    fn inject_cc_tpl_skips_when_in_to_among_others() {
        let original = format!(
            "From: a@example.com\nTo: other@x.com, {}\nSubject: hi\n\nBody",
            CC_EMAIL
        );
        let mut tpl = original.clone();
        inject_cc_in_tpl(&mut tpl);
        assert_eq!(tpl, original);
    }

    // --- inject_missing_headers tests ---

    #[test]
    fn inject_headers_adds_all_when_missing() {
        let msg = b"From: a@example.com\r\nSubject: hi\r\n\r\nBody text";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("MIME-Version: 1.0"));
        // Original headers preserved
        assert!(text.contains("From: a@example.com"));
        assert!(text.contains("Subject: hi"));
    }

    #[test]
    fn inject_headers_replaces_existing() {
        let msg = b"From: a@example.com\r\nDate: Thu, 01 Jan 1970 00:00:00 +0000\r\nMessage-ID: <old@old>\r\nMIME-Version: 1.0\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Old values should be gone
        assert!(!text.contains("Thu, 01 Jan 1970"));
        assert!(!text.contains("<old@old>"));
        // Fresh values should be present
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("MIME-Version: 1.0"));
    }

    #[test]
    fn inject_headers_preserves_body() {
        let body = "This is the body\r\nwith multiple lines\r\nand stuff.";
        let msg = format!("From: a@example.com\r\nSubject: hi\r\n\r\n{}", body);
        let result = inject_missing_headers(msg.as_bytes());
        let text = String::from_utf8(result).unwrap();
        assert!(text.ends_with(body));
    }

    #[test]
    fn inject_headers_handles_folded() {
        let msg = b"From: a@example.com\r\nDate: Thu,\r\n 01 Jan 1970 00:00:00 +0000\r\nMessage-ID:\r\n <old@old>\r\nSubject: hi\r\n\r\nBody";
        let result = inject_missing_headers(msg);
        let text = String::from_utf8(result).unwrap();
        // Folded old values should be fully removed
        assert!(!text.contains("01 Jan 1970"));
        assert!(!text.contains("<old@old>"));
        // Fresh values present
        assert!(text.contains("Date: "));
        assert!(text.contains("Message-ID: <"));
        assert!(text.contains("MIME-Version: 1.0"));
        assert!(text.contains("Body"));
    }
}
