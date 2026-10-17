use anyhow::Result;
use bytes::Bytes;

use crate::http::{HttpClient, HttpRequest, HttpResponse};
use wacore::net::StreamingHttpResponse;

/// Isolates streaming crypto and output accounting from async scheduling and HTTP.
pub struct StreamingDecryptHarness {
    encrypted: wacore::upload::EncryptedMedia,
    plaintext: Vec<u8>,
}

impl StreamingDecryptHarness {
    pub fn new(size: usize) -> Result<Self> {
        let plaintext = vec![7; size];
        let encrypted =
            wacore::upload::encrypt_media(&plaintext, wacore::download::MediaType::AppState)?;
        Ok(Self {
            encrypted,
            plaintext,
        })
    }

    fn decrypt(&self, writer: &mut impl std::io::Write) -> Result<()> {
        wacore::download::DownloadUtils::decrypt_stream_to_writer_with_hashes(
            std::io::Cursor::new(&self.encrypted.data_to_upload),
            &self.encrypted.media_key,
            wacore::download::MediaType::AppState,
            Some(&self.encrypted.file_enc_sha256),
            Some(&self.encrypted.file_sha256),
            writer,
        )?;
        Ok(())
    }

    pub fn decrypt_vec(&self) -> Result<()> {
        let mut output = Vec::with_capacity(self.plaintext.len());
        self.decrypt(&mut output)?;
        anyhow::ensure!(output == self.plaintext, "streaming plaintext mismatch");
        Ok(())
    }

    pub fn decrypt_accounted(&self) -> Result<()> {
        let budget = crate::download::memory::MemoryBudget::new(self.plaintext.len());
        let mut output = crate::download::memory::MemoryWriter::bounded(
            self.plaintext.len() as u64,
            budget,
        )?;
        self.decrypt(&mut output)?;
        anyhow::ensure!(
            output.into_bytes().as_ref() == self.plaintext,
            "accounted streaming plaintext mismatch"
        );
        Ok(())
    }
}

/// Immutable encrypted input shared by native media benchmarks.
pub struct StreamingBodyHttp(Bytes);

impl StreamingBodyHttp {
    pub fn new(body: Bytes) -> Self {
        Self(body)
    }
}

#[async_trait::async_trait]
impl HttpClient for StreamingBodyHttp {
    async fn execute(&self, _: HttpRequest) -> Result<HttpResponse> {
        anyhow::bail!("benchmark expects streaming HTTP")
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn execute_streaming(&self, _: HttpRequest) -> Result<StreamingHttpResponse> {
        Ok(StreamingHttpResponse::new(
            200,
            Box::new(std::io::Cursor::new(self.0.clone())),
        ))
    }
}
