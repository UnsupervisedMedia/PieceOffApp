use crate::valid_media_extension;

/// A 64-character hexadecimal SHA-256 digest.
pub fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn clean_name(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(24).collect();
    if name.trim().is_empty() {
        "Guest".into()
    } else {
        name.trim().to_owned()
    }
}

pub fn clean_title(title: &str) -> String {
    let title: String = title.chars().filter(|c| !c.is_control()).take(80).collect();
    if title.trim().is_empty() {
        "Untitled puzzle".into()
    } else {
        title.trim().to_owned()
    }
}

pub fn clean_media_extension(extension: &str) -> Result<String, String> {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    if valid_media_extension(&extension).is_some() {
        Ok(extension)
    } else {
        Err("Choose a supported image or video format".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_bounded_and_control_characters_removed() {
        assert_eq!(clean_name("\nAlice\0"), "Alice");
        assert_eq!(clean_name(" \t "), "Guest");
        assert!(clean_name(&"x".repeat(100)).len() <= 24);
    }

    #[test]
    fn names_are_bounded_by_characters_not_bytes() {
        let long = "é".repeat(100);
        assert_eq!(clean_name(&long).chars().count(), 24);
    }

    #[test]
    fn room_titles_are_bounded() {
        assert_eq!(clean_title(" \n "), "Untitled puzzle");
        assert!(clean_title(&"x".repeat(200)).len() <= 80);
    }

    #[test]
    fn media_extension_accepts_the_shared_set() {
        for extension in crate::SUPPORTED_MEDIA_EXTENSIONS {
            assert_eq!(clean_media_extension(extension).unwrap(), *extension);
        }
        assert_eq!(clean_media_extension(".APNG").unwrap(), "apng");
        assert!(clean_media_extension("../mp4").is_err());
        assert!(clean_media_extension("exe").is_err());
    }
}
