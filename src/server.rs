use crate::{Filter, Message};
use bytes::Bytes;
use futures::StreamExt;
use hyper::http::{header, Request};
use hyper::{Body, Method, Response, StatusCode};
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::{broadcast, RwLock};
use tokio::task::JoinHandle;
use tokio_stream::wrappers::BroadcastStream;

#[derive(Debug)]
struct Record {
    ttaaii: String,
    cccc: String,
    awips_id: Option<String>,
    id: Option<String>,
    sse_raw: Bytes,
}

impl<'a> From<&'a Record> for crate::FilterItem<'a> {
    fn from(r: &'a Record) -> Self {
        Self {
            ttaaii: &r.ttaaii,
            cccc: &r.cccc,
            awips_id: r.awips_id.as_ref().map(String::as_str),
        }
    }
}

impl From<nwws_oi::Message> for Record {
    fn from(m: nwws_oi::Message) -> Self {
        crate::Message::from(m).into()
    }
}

impl From<crate::Message> for Record {
    fn from(m: Message) -> Self {
        let mut buffer = Vec::with_capacity(m.message.len() * 9 / 8 + 20);
        use std::io::Write;

        buffer.write(b"data:").unwrap();
        // guaranteed not to add newlines:
        serde_json::to_writer(&mut buffer, &m).expect("serialize");

        if let Some(id) = &m.nwws_oi_id {
            buffer.write(b"\nid:").unwrap();
            buffer.write(id.as_bytes()).unwrap();
        }
        buffer.write(b"\n\n").unwrap();

        buffer.shrink_to_fit();

        let sse_raw = Bytes::from(buffer);

        Self {
            ttaaii: m.ttaaii,
            cccc: m.cccc,
            awips_id: m.awips_id,
            id: m.nwws_oi_id,
            sse_raw,
        }
    }
}

#[derive(Debug)]
pub struct Endpoint {
    _task: JoinHandle<()>,
    recent: Arc<RwLock<VecDeque<Arc<Record>>>>,
    broadcast: broadcast::Sender<Arc<Record>>,
}

impl Endpoint {
    pub fn new<S, I>(source: S) -> Self
    where
        S: futures::Stream<Item = I> + Send + 'static,
        I: TryInto<Message> + Send,
        I::Error: std::error::Error + Send,
    {
        let (broadcast, _) = broadcast::channel(20);
        let recent = Arc::new(RwLock::new(VecDeque::with_capacity(100)));

        let task = tokio::task::spawn(run(source, recent.clone(), broadcast.clone()));

        Self {
            _task: task,
            recent,
            broadcast,
        }
    }

    pub async fn handle_request(
        &self,
        request: Request<Body>,
    ) -> hyper::http::Result<Response<Body>> {
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
            /*
            _ => error_response(
                StatusCode::NOT_ACCEPTABLE,
                "Accept: header must request either SSE or JSON\n",
            ),
             */
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

        let ids = {
            let recent = self.recent.read().await;
            recent.iter().map(|i| i.id.clone()).collect()
        };

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
        let rx = self.broadcast.subscribe().into();

        let last_event_id = if let Some(last_event_id) = request
            .headers()
            .get(header::HeaderName::from_static("last-event-id"))
        {
            Some(String::from_utf8_lossy(last_event_id.as_bytes()).into_owned())
        } else {
            None
        };

        let queue = {
            let recent = self.recent.read().await;
            let queue = if let Some(id) = last_event_id {
                // find it in the buffer
                let mut after = recent.iter().peekable();
                while let Some(_) = after.next_if(|r| r.id.as_ref() != Some(&id)) {}
                let after: Vec<_> = after.cloned().collect();
                if after.is_empty() {
                    // send all recent messages
                    recent.iter().cloned().collect()
                } else {
                    // send the ones we found
                    after
                }
            } else {
                // send the last 5
                recent.iter().rev().take(5).rev().cloned().collect()
            };
            queue.into_iter()
        };

        let builder = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
            .header(header::CACHE_CONTROL, "private, no-cache");

        builder.body(Body::wrap_stream(StreamBody { rx, filter, queue }))
    }
}

struct StreamBody {
    rx: BroadcastStream<Arc<Record>>,
    filter: Filter,
    queue: std::vec::IntoIter<Arc<Record>>,
}

impl futures::Stream for StreamBody {
    type Item = hyper::http::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let record = if let Some(q) = self.queue.next() {
            q
        } else {
            loop {
                match Pin::new(&mut self.rx).poll_next(cx) {
                    Poll::Ready(Some(Ok(record))) => {
                        if self.filter.matches(record.as_ref()) {
                            break record;
                        } else {
                            // receive again
                        }
                    }
                    Poll::Ready(_) => {
                        // client has lagged out
                        return Poll::Ready(None);
                    }
                    Poll::Pending => {
                        return Poll::Pending;
                    }
                }
            }
        };

        Poll::Ready(Some(Ok(record.sse_raw.clone())))
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

async fn run<S>(
    stream: S,
    recent: Arc<RwLock<VecDeque<Arc<Record>>>>,
    broadcast: broadcast::Sender<Arc<Record>>,
) where
    S: futures::Stream + Send + 'static,
    <S as futures::Stream>::Item: TryInto<Message>,
    <<S as futures::Stream>::Item as TryInto<Message>>::Error: std::error::Error,
{
    let mut stream = Box::pin(stream);
    while let Some(event) = stream.next().await {
        match event.try_into() {
            Ok(message) => {
                // Convert
                let record = Arc::new(Record::from(message));

                // Add to recent messages
                let mut recent = recent.write().await;
                if recent.capacity() == recent.len() {
                    recent.pop_front();
                }
                recent.push_back(record.clone());
                std::mem::drop(recent);

                // Broadcast
                // (Failures mean "no receiver", not "no one will ever receive", so ignore)
                broadcast.send(record).ok();
            }
            Err(e) => {
                log::error!("stream error: {}", e);
            }
        }
    }
    panic!("stream ended");
}
