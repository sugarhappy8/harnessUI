//! SPIKE (temporary, replaced before the feature is finished): serves files from the folder
//! named by `ARTIFACT_SPIKE_DIR` over the `artifact` scheme, with the headers written next
//! to each file, so the isolation of a framed page can be measured on this platform.

use tauri::http::{Request, Response, StatusCode};
use tauri::{Runtime, UriSchemeContext, UriSchemeResponder};

pub const SCHEME: &str = "artifact";

pub fn is_frame_navigation(url: &url::Url) -> bool {
    url.scheme() == SCHEME
}

pub fn serve<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let label = ctx.webview_label().to_string();
    let Ok(dir) = std::env::var("ARTIFACT_SPIKE_DIR") else {
        responder.respond(
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(b"no spike dir".to_vec())
                .unwrap_or_default(),
        );
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let uri = request.uri().to_string();
    let path = request.uri().path().trim_start_matches('/').to_string();
    let headers: Vec<String> = request
        .headers()
        .iter()
        .map(|(k, v)| format!("{k}: {}", v.to_str().unwrap_or("?")))
        .collect();
    let line = format!(
        "{} {} [{}] webview={} headers={:?}\n",
        request.method(),
        uri,
        path,
        label,
        headers
    );
    {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("requests.log"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
    let name = path.replace('/', "_");
    let file = dir.join(&name);
    let Ok(body) = std::fs::read(&file) else {
        responder.respond(
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header("Content-Type", "text/plain")
                .body(b"not found".to_vec())
                .unwrap_or_default(),
        );
        return;
    };
    let mut builder = Response::builder().status(StatusCode::OK);
    let header_file = dir.join(format!("{name}.headers"));
    if let Ok(text) = std::fs::read_to_string(header_file) {
        for l in text.lines() {
            if let Some((k, v)) = l.split_once(": ") {
                builder = builder.header(k.trim(), v.trim());
            }
        }
    }
    responder.respond(builder.body(body).unwrap_or_default());
}
