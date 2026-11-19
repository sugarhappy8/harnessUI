//! A `multipart/form-data` body (RFC 7578), written by hand for OpenAI's picture edits so
//! that no new dependency is needed. Every value is written as given; the boundary is checked
//! against each part, its headers included, and never occurs in one.

/// A file sent as one part of a form.
pub(crate) struct FilePart {
    /// The form field's name, for example `image[]`.
    pub name: String,
    /// The file name sent with it, for example `picture-1.png`.
    pub filename: String,
    /// Its media type, for example `image/png`.
    pub mime: String,
    /// The file's bytes.
    pub bytes: Vec<u8>,
}

/// The form with these text fields, then these files, in order: the boundary and the body.
pub(crate) fn form(fields: &[(&str, String)], files: &[FilePart]) -> (String, Vec<u8>) {
    form_with(fields, files, || {
        format!("harnessui-{}", uuid::Uuid::new_v4().simple())
    })
}

/// [`form`] with the boundaries to try: the first that occurs in no part is used.
fn form_with(
    fields: &[(&str, String)],
    files: &[FilePart],
    mut next_boundary: impl FnMut() -> String,
) -> (String, Vec<u8>) {
    // Everything written between two boundaries, headers included.
    let parts = || {
        fields
            .iter()
            .flat_map(|(name, value)| [name.as_bytes(), value.as_bytes()])
            .chain(files.iter().flat_map(|file| {
                [
                    file.name.as_bytes(),
                    file.filename.as_bytes(),
                    file.mime.as_bytes(),
                    file.bytes.as_slice(),
                ]
            }))
    };
    let boundary = loop {
        let candidate = next_boundary();
        if !parts().any(|part| contains(part, candidate.as_bytes())) {
            break candidate;
        }
    };
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n",
                escaped(name)
            )
            .as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    for file in files {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n\
                 Content-Type: {}\r\n\r\n",
                escaped(&file.name),
                escaped(&file.filename),
                escaped(&file.mime),
            )
            .as_bytes(),
        );
        body.extend_from_slice(&file.bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// Text for a part's header line: `"`, CR and LF escaped as the HTML standard writes them in
/// names (`%22`, `%0D`, `%0A`), so no text can end its quotes or its line.
fn escaped(text: &str) -> String {
    text.replace('"', "%22")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Whether `needle` occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(name: &str, mime: &str, bytes: &[u8]) -> FilePart {
        FilePart {
            name: "image[]".to_owned(),
            filename: name.to_owned(),
            mime: mime.to_owned(),
            bytes: bytes.to_vec(),
        }
    }

    #[test]
    fn a_multipart_body_has_each_part() {
        let png = b"\x89PNG\r\n\x1a\n first";
        let webp = b"RIFF\x10\0\0\0WEBPVP8 second";
        let prompt = "a fox\r\non two lines";
        let (boundary, body) = form(
            &[
                ("model", "gpt-image-2.6-flare".to_owned()),
                ("prompt", prompt.to_owned()),
            ],
            &[
                picture("picture-1.png", "image/png", png),
                picture("picture-2.webp", "image/webp", webp),
            ],
        );
        for part in [
            b"gpt-image-2.6-flare".as_slice(),
            prompt.as_bytes(),
            png.as_slice(),
            webp.as_slice(),
        ] {
            assert!(!contains(part, boundary.as_bytes()), "{boundary}");
        }
        assert!(
            boundary.len() <= 70,
            "RFC 2046 allows at most 70 characters"
        );

        let mut expected = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n\
             gpt-image-2.6-flare\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\n\
             {prompt}\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; \
             filename=\"picture-1.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .into_bytes();
        expected.extend_from_slice(png);
        expected.extend(
            format!(
                "\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; \
                 filename=\"picture-2.webp\"\r\nContent-Type: image/webp\r\n\r\n"
            )
            .into_bytes(),
        );
        expected.extend_from_slice(webp);
        expected.extend(format!("\r\n--{boundary}--\r\n").into_bytes());
        assert_eq!(body, expected);
    }

    #[test]
    fn a_boundary_found_in_a_part_is_never_used() {
        let mut tries = ["fox", "dusk", "harbour"].into_iter().map(str::to_owned);
        let (boundary, _) = form_with(&[("prompt", "a fox at dusk".to_owned())], &[], || {
            tries.next().expect("another boundary")
        });
        assert_eq!(boundary, "harbour");
    }

    #[test]
    fn quotes_and_line_breaks_in_names_are_escaped() {
        let (boundary, body) = form(
            &[],
            &[picture("say \"hi\"\r\n.png", "image/png", b"\x89PNG")],
        );
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("filename=\"say %22hi%22%0D%0A.png\"\r\n"),
            "{text}"
        );
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
    }
}
