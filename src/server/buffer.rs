use crate::{Filter, Message};
use bytes::Bytes;
use futures::StreamExt;
use hyper::server::accept::Accept;
use hyper::HeaderMap;
use std::collections::VecDeque;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::BroadcastStream;

/// A small in-memory buffer of NWWS messages suitable for pub/sub dissemination.
#[derive(Debug)]
pub(crate) struct Buffer {
    _task: tokio::task::JoinHandle<()>,
    recent: Arc<RwLock<VecDeque<Arc<Entry>>>>,
    broadcast: broadcast::Sender<Arc<Entry>>,
}

#[derive(Debug, Clone)]
pub(crate) struct Subscription {
    pub last_event_id: Option<String>,
    pub filter: crate::Filter,
    pub content_type: ContentType,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ContentType {
    EventStream,
    NdJson,
}

impl<'a> From<&hyper::HeaderMap> for ContentType {
    fn from(m: &hyper::HeaderMap) -> Self {
        match m
            .get(hyper::header::ACCEPT)
            .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
        {
            Some(v) if v.contains("ndjson") => ContentType::NdJson,
            _ => ContentType::EventStream,
        }
    }
}

impl From<ContentType> for &'static str {
    fn from(ct: ContentType) -> Self {
        match ct {
            ContentType::EventStream => "text/event-stream; charset=utf-8",
            ContentType::NdJson => "application/x-ndjson; charset=utf-8",
        }
    }
}

impl Buffer {
    pub fn new<S, E>(source: S) -> Self
    where
        S: futures::Stream<Item = Result<crate::Message, E>> + Send + 'static,
        E: std::error::Error + Send + 'static,
    {
        let (broadcast, _) = broadcast::channel(20);
        let recent = Arc::new(RwLock::new(VecDeque::with_capacity(100)));

        let _task = tokio::task::spawn(run(source, recent.clone(), broadcast.clone()));
        Self {
            _task,
            recent,
            broadcast,
        }
    }

    pub async fn ids(&self) -> Vec<Option<String>> {
        let recent = self.recent.read().await;
        recent.iter().map(|rec| rec.id.clone()).collect()
    }

    pub async fn subscribe(
        &self,
        subscription: Subscription,
    ) -> impl futures::stream::Stream<Item = hyper::http::Result<bytes::Bytes>> {
        let Subscription {
            filter,
            last_event_id,
            content_type,
        } = subscription;

        let rx = self.broadcast.subscribe().into();

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

        Receiver {
            rx,
            filter,
            queue,
            content_type,
        }
    }
}

struct Receiver {
    rx: BroadcastStream<Arc<Entry>>,
    filter: Filter,
    queue: std::vec::IntoIter<Arc<Entry>>,
    content_type: ContentType,
}

impl futures::Stream for Receiver {
    type Item = hyper::http::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let entry = if let Some(q) = self.queue.next() {
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

        let bytes = match self.content_type {
            ContentType::EventStream => entry.sse_raw.clone(),
            ContentType::NdJson => entry.sse_raw.slice(entry.json_nd_range.clone()),
        };

        Poll::Ready(Some(Ok(bytes)))
    }
}

async fn run<S, E>(
    stream: S,
    recent: Arc<RwLock<VecDeque<Arc<Entry>>>>,
    broadcast: broadcast::Sender<Arc<Entry>>,
) where
    S: futures::Stream<Item = Result<crate::Message, E>> + Send + 'static,
    E: std::error::Error + Send + 'static,
{
    let mut stream = Box::pin(stream);
    while let Some(event) = stream.next().await {
        match event {
            Ok(message) => {
                // Convert
                let record = Arc::new(Entry::from(message));

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

#[derive(Debug)]
struct Entry {
    ttaaii: String,
    cccc: String,
    awips_id: Option<String>,
    id: Option<String>,
    sse_raw: Bytes,
    json_nd_range: Range<usize>,
}

impl<'a> From<&'a Entry> for crate::FilterItem<'a> {
    fn from(r: &'a Entry) -> Self {
        Self {
            ttaaii: &r.ttaaii,
            cccc: &r.cccc,
            awips_id: r.awips_id.as_ref().map(String::as_str),
        }
    }
}

impl From<crate::Message> for Entry {
    fn from(m: Message) -> Self {
        let mut server_sent_event = Vec::with_capacity(m.message.len() * 9 / 8 + 20);
        use std::io::Write;

        server_sent_event.write(b"data:").unwrap();
        let json_start_at = server_sent_event.len();
        // guaranteed not to add newlines:
        serde_json::to_writer(&mut server_sent_event, &m).expect("serialize");
        let json_end_at = server_sent_event.len();

        if let Some(id) = &m.nwws_oi_id {
            server_sent_event.write(b"\nid:").unwrap();
            server_sent_event.write(id.as_bytes()).unwrap();
        }
        server_sent_event.write(b"\n\n").unwrap();

        // no matter what, the byte after json_end_at is a newline
        let json_nd_range = (json_start_at..json_end_at + 1);

        server_sent_event.shrink_to_fit();

        let sse_raw = Bytes::from(server_sent_event);

        Self {
            ttaaii: m.ttaaii,
            cccc: m.cccc,
            awips_id: m.awips_id,
            id: m.nwws_oi_id,
            sse_raw,
            json_nd_range: json_nd_range,
        }
    }
}
