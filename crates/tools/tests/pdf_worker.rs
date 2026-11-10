//! The PDF worker end to end: this test binary doubles as the worker (as the app binary
//! does), so extraction really runs in a separate process.

use std::time::Duration;

/// A minimal one-page PDF with a line of text, xref offsets computed.
fn tiny_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 18 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
    ];
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for o in offsets {
        out.extend(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(tools::pdf::WORKER_FLAG) {
        tools::pdf::worker_main(args.get(2).map(String::as_str).unwrap_or(""));
    }
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = std::env::current_exe().expect("exe");

        let good = dir.path().join("hello.pdf");
        std::fs::write(&good, tiny_pdf("Hello PDF world")).expect("write");
        let text = tools::pdf::extract_text(&exe, &good, Duration::from_secs(20))
            .await
            .expect("extract");
        assert!(text.contains("Hello PDF world"), "got {text:?}");

        let bad = dir.path().join("bad.pdf");
        std::fs::write(&bad, b"%PDF-1.4\nnot really a pdf").expect("write");
        let err = tools::pdf::extract_text(&exe, &bad, Duration::from_secs(20))
            .await
            .expect_err("bad pdf");
        assert!(matches!(err, tools::pdf::PdfError::Unreadable), "{err:?}");
        println!("pdf worker: ok");
    });
}
