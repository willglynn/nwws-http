use crate::server::gzip;
use crate::server::gzip::Block;
use crate::{Filter, Message};
use bytes::Bytes;
use futures::StreamExt;
use miniz_oxide::deflate::CompressionLevel;
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
    pub content_encoding: ContentEncoding,
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

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum ContentEncoding {
    Identity,
    Gzip,
}

impl<'a> From<&hyper::HeaderMap> for ContentEncoding {
    fn from(m: &hyper::HeaderMap) -> Self {
        match m
            .get(hyper::header::ACCEPT_ENCODING)
            .and_then(|h| std::str::from_utf8(h.as_bytes()).ok())
        {
            Some(v) if v.contains("gzip") => ContentEncoding::Gzip,
            _ => ContentEncoding::Identity,
        }
    }
}

impl From<ContentEncoding> for &'static str {
    fn from(ct: ContentEncoding) -> Self {
        match ct {
            ContentEncoding::Identity => "identity",
            ContentEncoding::Gzip => "gzip",
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
            content_encoding,
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

        let filter_is_empty = filter.is_empty();
        let receiver = Receiver { rx, filter, queue };

        match content_encoding {
            ContentEncoding::Identity => receiver
                .map(move |e| e.map(|entry| entry.bytes_for(content_type)))
                .boxed(),
            ContentEncoding::Gzip => {
                if filter_is_empty {
                    let mut transform = Some(gzip::Transform::new(CompressionLevel::DefaultLevel));
                    receiver
                        .map(move |e| {
                            e.map(|entry| {
                                let block = entry.gzip_block_for(content_type);
                                match (block, transform.as_mut()) {
                                    (Block::First(bytes), _) => {
                                        log::trace!("joined the mainline");
                                        transform = None;
                                        bytes.clone()
                                    }
                                    (Block::Middle(bytes), None) => bytes.clone(),
                                    (Block::Middle(main), Some(x)) => {
                                        let input = entry.bytes_for(content_type);
                                        let block =
                                            x.write_block(input.as_ref(), false).expect("compress");
                                        let output = Bytes::from(block);
                                        log::trace!(
                                            "{} -> {} local vs {} -> {} main",
                                            input.len(),
                                            output.len(),
                                            input.len(),
                                            main.len()
                                        );
                                        output
                                    }
                                }
                            })
                        })
                        .boxed()
                } else {
                    super::gzip::Stream::new(
                        receiver.map(move |r| r.map(|e| e.bytes_for(content_type))),
                        CompressionLevel::DefaultLevel,
                    )
                    .map(|b| b.map(Bytes::from))
                    .boxed()
                }
            }
        }
    }
}

struct Receiver {
    rx: BroadcastStream<Arc<Entry>>,
    filter: Filter,
    queue: std::vec::IntoIter<Arc<Entry>>,
}

impl futures::Stream for Receiver {
    type Item = hyper::http::Result<Arc<Entry>>;

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

        Poll::Ready(Some(Ok(entry)))
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
    let mut sse_gzip = gzip::Transform::new(CompressionLevel::UberCompression);
    let mut sse_ndjson = gzip::Transform::new(CompressionLevel::BestCompression);

    let mut bytes_since_flushing = 0;

    while let Some(event) = stream.next().await {
        match event {
            Ok(message) => {
                // flush every 4 MB
                let flush_compressor_state = if bytes_since_flushing > 4 << 20 {
                    bytes_since_flushing = 0;
                    log::trace!("🚽");
                    true
                } else {
                    false
                };

                // Convert
                let record = Arc::new(Entry::new(
                    message,
                    flush_compressor_state,
                    &mut sse_gzip,
                    &mut sse_ndjson,
                ));
                bytes_since_flushing += record.sse_raw.len();

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

    /// The bytes which represent this message encoded as Server-Sent Events
    sse_raw: Bytes,

    /// The range within `sse_raw` which makes an ndjson message
    ndjson_range: Range<usize>,

    /// `sse_raw`, pre-compressed
    sse_gzip: gzip::Block,

    /// `&sse_raw[ndjson_range]`, pre-compressed
    ndjson_gzip: gzip::Block,
}

impl Entry {
    fn new(
        m: Message,
        flush_compressor_state: bool,
        sse_gzip: &mut gzip::Transform,
        ndjson_gzip: &mut gzip::Transform,
    ) -> Self {
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
        let json_nd_range = json_start_at..json_end_at + 1;

        server_sent_event.shrink_to_fit();

        let sse_raw = Bytes::from(server_sent_event);
        let sse_gzip = sse_gzip
            .write_block(&sse_raw, flush_compressor_state)
            .expect("compress");

        let ndjson = &sse_raw[json_nd_range.clone()];
        let ndjson_gzip = ndjson_gzip
            .write_block(ndjson, flush_compressor_state)
            .expect("compress");

        Self {
            ttaaii: m.ttaaii,
            cccc: m.cccc,
            awips_id: m.awips_id,
            id: m.nwws_oi_id,
            sse_raw,
            ndjson_range: json_nd_range,
            sse_gzip,
            ndjson_gzip,
        }
    }

    fn bytes_for(&self, content_type: ContentType) -> Bytes {
        match content_type {
            ContentType::EventStream => self.sse_raw.clone(),
            ContentType::NdJson => self.sse_raw.slice(self.ndjson_range.clone()),
        }
    }

    fn gzip_block_for(&self, content_type: ContentType) -> &gzip::Block {
        match content_type {
            ContentType::EventStream => &self.sse_gzip,
            ContentType::NdJson => &self.ndjson_gzip,
        }
    }
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
