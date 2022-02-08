use futures::{StreamExt, TryStreamExt};
use hyper::Body;
use std::error::Error;
use std::pin::Pin;
use std::task::{Context, Poll};

pub struct Client {
    inner: futures::stream::BoxStream<'static, Result<crate::Message, ClientError>>,
}

impl Client {
    pub fn new<C>(client: hyper::client::Client<C, Body>, uri: hyper::Uri) -> Self
    where
        C: hyper::client::connect::Connect + Clone + Send + Sync + 'static,
    {
        Self {
            inner: futures::stream::repeat_with(move || Request::new(&client, uri.clone()))
                .flatten()
                .boxed(),
        }
    }

    pub fn for_uri(uri: hyper::Uri) -> Self {
        Self::new(
            hyper::client::Client::builder().build(hyper_tls::HttpsConnector::new()),
            uri,
        )
    }
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ClientError {
    HttpRequest(hyper::Error),
    EventStream(Box<dyn std::error::Error + Send + Sync + 'static>),
    MessageFormat(Option<String>, serde_json::Error),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self {
            ClientError::HttpRequest(e) => write!(f, "HTTP request failed: {}", e),
            ClientError::EventStream(e) => write!(f, "event stream error: {}", e),
            ClientError::MessageFormat(id, err) => {
                write!(f, "invalid message format in ID={:?}: {}", id, err)
            }
        }
    }
}
impl std::error::Error for ClientError {
    fn cause(&self) -> Option<&dyn Error> {
        match self {
            ClientError::HttpRequest(e) => Some(e),
            ClientError::EventStream(e) => Some(e.as_ref()),
            ClientError::MessageFormat(_, e) => Some(e),
        }
    }
}

impl futures::Stream for Client {
    type Item = Result<crate::Message, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

struct Request {
    inner: futures::stream::BoxStream<'static, Result<crate::Message, ClientError>>,
}

impl Request {
    fn new<C>(client: &hyper::client::Client<C>, uri: hyper::Uri) -> Self
    where
        C: hyper::client::connect::Connect + Clone + Send + Sync + 'static,
    {
        let req = hyper::Request::builder()
            .uri(uri)
            .header(hyper::header::ACCEPT, "text/event-stream")
            .header(
                hyper::header::USER_AGENT,
                "https://github.com/willglynn/nwws-http",
            )
            .body(hyper::Body::from(""))
            .expect("build request");

        let inner = futures::stream::once(client.request(req))
            .flat_map(|result| match result {
                Ok(response) => {
                    log::info!("response: {:?}", response);
                    // todo: response.headers
                    let bytes = TryStreamExt::map_err(response.into_body(), |e| {
                        std::io::Error::new(std::io::ErrorKind::Other, e)
                    });

                    async_sse::decode(bytes.into_async_read())
                        .filter_map(|decode| {
                            futures::future::ready(match decode {
                                Ok(async_sse::Event::Retry(_)) => {
                                    // ignore
                                    None
                                }
                                Ok(async_sse::Event::Message(m)) => {
                                    Some(match serde_json::from_slice(m.data()) {
                                        Ok(message) => Ok(message),
                                        Err(e) => {
                                            Err(ClientError::MessageFormat(m.id().clone(), e))
                                        }
                                    })
                                }
                                Err(e) => Some(Err(ClientError::EventStream(e.into()))),
                            })
                        })
                        .boxed()
                }
                Err(e) => {
                    // todo: wait for retry
                    futures::stream::once(futures::future::ready(Err(ClientError::HttpRequest(e))))
                        .boxed()
                }
            })
            .boxed();

        Self { inner }
    }
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("Request").finish_non_exhaustive()
    }
}

impl futures::Stream for Request {
    type Item = Result<crate::Message, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}
