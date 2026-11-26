//! Reads a Hugging Face file's tensor table with bounded HTTP Range requests, before any
//! download (design section 7). Every read goes through [`HfClient::fetch_head`], the guarded
//! fetcher: revision-pinned resolve URL, no redirect off Hugging Face's own hosts with the
//! token attached, and no more bytes taken than asked for even if the server ignores Range.

use crate::hf::{HfClient, HfError};
use crate::sd::index::{FileFormat, GGUF_HEADER_CAP, IndexError, TensorIndex, parse_prefix, sniff};

/// The first read: 1 MiB covers most GGUF tables and safetensors headers in one request.
pub const FIRST_READ: u64 = 1 << 20;
/// The most files read for one repo opened.
pub const MAX_FILES_PER_REPO: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum RemoteError {
    #[error(transparent)]
    Hf(#[from] HfError),
    #[error(transparent)]
    Index(#[from] IndexError),
}

/// The tensor index of `path` at `commit` in `repo`, which is `size` bytes long.
///
/// Reads `min(FIRST_READ, size)` bytes, then, while the header is cut off, reads again from
/// byte 0: safetensors for exactly the header (`8 + length`, at most two reads); GGUF for
/// `max(need, 2 x the last read)`, up to the 64 MiB cap (at most 8 reads, as `need` is only
/// a lower bound there). A header past its cap is [`IndexError::TooLarge`]; a file that ends
/// inside its header is [`IndexError::Corrupt`]. Hub refusals (a gated file's `NeedsToken`
/// or `NeedsTerms`) pass through unchanged.
pub async fn read_remote(
    hf: &HfClient,
    repo: &str,
    commit: &str,
    path: &str,
    size: u64,
) -> Result<TensorIndex, RemoteError> {
    let mut want = FIRST_READ.min(size);
    loop {
        let bytes = hf.fetch_head(repo, commit, path, want).await?;
        let got = bytes.len() as u64;
        match parse_prefix(&bytes) {
            Err(IndexError::Truncated { need }) => {
                let next = match sniff(&bytes) {
                    Some(FileFormat::Gguf) => need
                        .max(want.saturating_mul(2))
                        .min(GGUF_HEADER_CAP)
                        .min(size),
                    _ => need.min(size),
                };
                // The server gave less than asked (the file ended), or the read already
                // covers the cap or the whole file: nothing more to ask for.
                if got < want || next <= want {
                    return Err(
                        IndexError::Corrupt("the file ends inside its header".into()).into(),
                    );
                }
                want = next;
            }
            other => return other.map_err(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::index::{GGUF_HEADER_CAP, SAFETENSORS_HEADER_CAP};
    use crate::sd::testing::*;

    const COMMIT: &str = "c0ffee";

    #[tokio::test]
    async fn a_header_bigger_than_the_first_read_takes_one_more_request() {
        let file = safetensors(&big_header(2_000_000));
        let s = range_server(file.clone(), RangeMode::Honour).await;
        let ix = read_remote(
            &s.client(),
            "o/r",
            COMMIT,
            "m.safetensors",
            file.len() as u64,
        )
        .await
        .unwrap();
        assert_eq!(ix.format, FileFormat::Safetensors);
        assert_eq!(s.ranges(), ["bytes=0-1048575", "bytes=0-2000007"]);
    }

    #[tokio::test]
    async fn a_server_that_ignores_range_is_read_only_up_to_the_cap() {
        // a 300 MiB body whose header says 200 MiB: the client stops after the first 1 MiB and refuses
        let s = range_server(
            streamed_safetensors(200 << 20, 300 << 20),
            RangeMode::Ignore,
        )
        .await;
        let e = read_remote(&s.client(), "o/r", COMMIT, "m.safetensors", 300 << 20)
            .await
            .unwrap_err();
        assert!(matches!(
            e,
            RemoteError::Index(IndexError::TooLarge {
                cap: SAFETENSORS_HEADER_CAP
            })
        ));
        assert_eq!(s.ranges().len(), 1);
        // one socket buffer of slack
        assert!(
            s.bytes_sent() <= FIRST_READ + (4 << 20),
            "sent {}",
            s.bytes_sent()
        );
        // a 2 MB header behind a server that ignores Range still reads, in two bounded requests
        let file = safetensors(&big_header(2_000_000));
        let s = range_server(file.clone(), RangeMode::Ignore).await;
        assert!(
            read_remote(
                &s.client(),
                "o/r",
                COMMIT,
                "m.safetensors",
                file.len() as u64
            )
            .await
            .is_ok()
        );
        assert_eq!(s.ranges().len(), 2);
    }

    #[tokio::test]
    async fn a_gguf_table_past_64_mib_is_refused() {
        let s = range_server(
            gguf_claiming_table_bytes(65 << 20, 70 << 20),
            RangeMode::Honour,
        )
        .await;
        let e = read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 70 << 20).await;
        assert!(matches!(
            e,
            Err(RemoteError::Index(IndexError::TooLarge {
                cap: GGUF_HEADER_CAP
            }))
        ));
        // 1, 2, 4, 8, 16, 32 and 64 MiB: at most 8 reads.
        assert_eq!(s.ranges().len(), 7, "{:?}", s.ranges());
    }

    #[tokio::test]
    async fn a_gguf_header_of_8_mib_is_read_by_doubling() {
        // a 20 MiB file whose tensor table ends at 8,400,000 bytes: each truncated read asks
        // for one more entry, so the reads double
        let file = gguf_with_table_ending_at(8_400_000, 20 << 20);
        let s = range_server(file.clone(), RangeMode::Honour).await;
        assert!(
            read_remote(&s.client(), "o/r", COMMIT, "qwen3.gguf", file.len() as u64)
                .await
                .is_ok()
        );
        assert_eq!(
            s.ranges(),
            [
                "bytes=0-1048575",
                "bytes=0-2097151",
                "bytes=0-4194303",
                "bytes=0-8388607",
                "bytes=0-16777215"
            ]
        );
    }

    #[tokio::test]
    async fn a_file_that_ends_inside_its_header_is_corrupt() {
        // The header says 5 MB; the file is 3 MB.
        let mut file = safetensors(&big_header(2_000_000));
        file[..8].copy_from_slice(&5_000_000u64.to_le_bytes());
        file.resize(3_000_000, b' ');
        let s = range_server(file.clone(), RangeMode::Honour).await;
        let e = read_remote(
            &s.client(),
            "o/r",
            COMMIT,
            "m.safetensors",
            file.len() as u64,
        )
        .await;
        assert!(
            matches!(e, Err(RemoteError::Index(IndexError::Corrupt(_)))),
            "{e:?}"
        );
    }

    #[tokio::test]
    async fn a_gated_file_passes_the_hub_error_through() {
        let s = status_server(401).await;
        assert!(matches!(
            read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 10).await,
            Err(RemoteError::Hf(HfError::NeedsToken))
        ));
    }

    #[tokio::test]
    async fn a_redirect_off_the_allowed_hosts_is_never_followed() {
        let s = redirect_server("https://evil.example/m.gguf").await;
        assert!(matches!(
            read_remote(&s.client(), "o/r", COMMIT, "m.gguf", 10).await,
            Err(RemoteError::Hf(HfError::BlockedHost(_)))
        ));
    }
}
