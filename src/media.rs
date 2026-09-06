//! What kind of bytes these are — decided by looking at them.
//!
//! Never by the file extension and never by the content type the browser
//! announced: both are chosen by whoever is uploading. This is an **allow**
//! list of four formats, and the reason it is one is SVG, which is executable
//! XML that happens to be called an image.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Jpeg,
    Png,
    Gif,
    Webp,
}

impl MediaType {
    pub fn mime(&self) -> &'static str {
        match self {
            MediaType::Jpeg => "image/jpeg",
            MediaType::Png => "image/png",
            MediaType::Gif => "image/gif",
            MediaType::Webp => "image/webp",
        }
    }

    pub fn extension(&self) -> &'static str {
        match self {
            MediaType::Jpeg => "jpg",
            MediaType::Png => "png",
            MediaType::Gif => "gif",
            MediaType::Webp => "webp",
        }
    }

    pub fn from_mime(mime: &str) -> Option<Self> {
        match mime {
            "image/jpeg" => Some(MediaType::Jpeg),
            "image/png" => Some(MediaType::Png),
            "image/gif" => Some(MediaType::Gif),
            "image/webp" => Some(MediaType::Webp),
            _ => None,
        }
    }
}

/// The allow list, read from the first bytes.
pub fn detect(data: &[u8]) -> Option<MediaType> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(MediaType::Jpeg);
    }
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(MediaType::Png);
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some(MediaType::Gif);
    }
    if data.len() >= 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some(MediaType::Webp);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_four_allowed_formats() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(MediaType::Jpeg));
        assert_eq!(
            detect(b"\x89PNG\r\n\x1a\n....").map(|m| m.mime()),
            Some("image/png")
        );
        assert_eq!(
            detect(b"GIF89a.......").map(|m| m.mime()),
            Some("image/gif")
        );
        assert_eq!(
            detect(b"RIFF\x00\x00\x00\x00WEBPVP8 ").map(|m| m.mime()),
            Some("image/webp")
        );
    }

    #[test]
    fn refuses_svg_even_though_it_is_an_image() {
        // SVG is executable XML. It is the reason this list is an allow list
        // and not a deny list.
        assert_eq!(
            detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
            None
        );
        // Including with a byte-order mark or leading whitespace in front.
        assert_eq!(detect(b"\xef\xbb\xbf<svg>"), None);
        assert_eq!(detect(b"\n  <svg>"), None);
    }

    #[test]
    fn refuses_html_and_scripts() {
        assert_eq!(detect(b"<!DOCTYPE html><script>alert(1)</script>"), None);
        assert_eq!(detect(b"#!/bin/sh\nrm -rf /"), None);
        assert_eq!(detect(b"%PDF-1.7"), None);
    }

    #[test]
    fn a_png_header_on_html_content_is_still_only_a_png_header() {
        // Nothing is claimed about the rest of the file. It is served later
        // with a FIXED content type and nosniff, so a browser does not get to
        // reconsider.
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n<script>"), Some(MediaType::Png));
    }

    #[test]
    fn an_empty_or_short_file_is_refused_without_panicking() {
        assert_eq!(detect(b""), None);
        assert_eq!(detect(b"\x89PN"), None);
        assert_eq!(detect(b"RIFF"), None, "a truncated header must not index");
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00WEB"), None);
    }

    #[test]
    fn riff_without_webp_is_refused() {
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00AVI LIST"), None);
        assert_eq!(detect(b"RIFF\x00\x00\x00\x00WAVEfmt "), None);
    }

    #[test]
    fn the_extension_never_comes_from_the_upload() {
        // The stored name is built from what we detected, so ".php" or
        // "../../etc/passwd" in a file name cannot reach the disk.
        for (bytes, ext) in [(&b"\x89PNG\r\n\x1a\n"[..], "png"), (&b"GIF89a"[..], "gif")] {
            let detected = detect(bytes).expect("detected");
            assert!(detected.extension().chars().all(|c| c.is_ascii_lowercase()));
            assert_eq!(detected.extension(), ext);
        }
    }
}
