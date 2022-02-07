use hyper::http::{header, Request};
use hyper::{Body, Method, Response, StatusCode};

mod buffer;
use buffer::{Buffer, Subscription};

#[derive(Debug)]
pub struct Server {
    buffer: self::buffer::Buffer,
}

impl Server {
    pub fn new<S, E>(source: S) -> Self
    where
        S: futures::Stream<Item = Result<crate::Message, E>> + Send + 'static,
        E: std::error::Error + Send + 'static,
    {
        Self {
            buffer: Buffer::new(source),
        }
    }

    pub async fn stream(&self, request: Request<Body>) -> hyper::http::Result<Response<Body>> {
        if request.method() != Method::GET {
            return error_response(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
        }

        match request
            .headers()
            .get(header::ACCEPT)
            .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
        {
            Some(value) if value.contains("text/event-stream") => {
                self.handle_sse_request(request).await
            }
            Some(value) if value.contains("application/json") || value.contains("text/json") => {
                self.handle_json_request(request).await
            }
            _ => self.handle_sse_request(request).await,
        }
    }

    async fn handle_json_request(
        &self,
        _request: Request<Body>,
    ) -> hyper::http::Result<Response<Body>> {
        #[derive(serde::Serialize)]
        #[serde(rename = "camelCase")]
        struct Json {
            ids: Vec<Option<String>>,
        }

        let ids = self.buffer.ids().await;

        let response = Json { ids };
        let response = serde_json::to_vec(&response).expect("serialize");

        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
            .header(header::CACHE_CONTROL, "must-revalidate")
            .body(Body::from(response))
    }

    async fn handle_sse_request(
        &self,
        request: Request<Body>,
    ) -> hyper::http::Result<Response<Body>> {
        let filter = crate::Filter::from(request.uri());

        let last_event_id = if let Some(last_event_id) = request
            .headers()
            .get(header::HeaderName::from_static("last-event-id"))
        {
            Some(String::from_utf8_lossy(last_event_id.as_bytes()).into_owned())
        } else {
            None
        };

        let content_type = crate::server::buffer::ContentType::from(request.headers());

        let body = self
            .buffer
            .subscribe(Subscription {
                last_event_id,
                filter,
                content_type,
            })
            .await;

        let builder = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, <&str>::from(content_type))
            .header(header::CACHE_CONTROL, "private, no-cache");

        builder.body(Body::wrap_stream(body))
    }
}

fn error_response(
    status_code: StatusCode,
    message: &'static str,
) -> hyper::http::Result<Response<Body>> {
    Response::builder()
        .status(status_code)
        .body(Body::from(message))
}
