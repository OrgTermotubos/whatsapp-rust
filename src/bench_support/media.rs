use anyhow::Result;
use bytes::Bytes;

use crate::http::{HttpClient, HttpRequest, HttpResponse};
use wacore::net::StreamingHttpResponse;

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
