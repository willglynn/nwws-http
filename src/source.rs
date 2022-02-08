use crate::client::Client;
use crate::NwwsOiStream;
use std::pin::Pin;
use std::task::Poll;

#[derive(Debug)]
pub enum Source {
    #[cfg(feature = "client")]
    Http(Client),
    #[cfg(feature = "nwws-oi")]
    NwwsOi(crate::NwwsOiStream),
}

impl Source {
    pub fn from_env() -> Self {
        if let Some(uri) = std::env::var("NWWS_HTTP_URI")
            .ok()
            .map(|s| hyper::Uri::try_from(s).expect("NWWS_HTTP_URI must be a valid URI if set"))
        {
            Source::Http(Client::for_uri(uri))
        } else if let (Ok(user), Ok(pass)) = (
            std::env::var("NWWS_OI_USERNAME"),
            std::env::var("NWWS_OI_PASSWORD"),
        ) {
            Source::NwwsOi(NwwsOiStream::new((user, pass)))
        } else {
            log::info!("defaulting to NWWS_HTTP_URI=https://nwws-http.fly.dev/stream");
            Source::Http(Client::for_uri(hyper::Uri::from_static(
                "https://nwws-http.fly.dev/stream",
            )))
        }
    }
}

impl futures::Stream for Source {
    type Item = Result<crate::Message, SourceError>;

    fn poll_next(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Option<Self::Item>> {
        match self.get_mut() {
            #[cfg(feature = "client")]
            Source::Http(s) => match Pin::new(s).poll_next(cx) {
                Poll::Ready(Some(Ok(m))) => Poll::Ready(Some(Ok(m.into()))),
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(SourceError(Box::new(e))))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
            #[cfg(feature = "nwws-oi")]
            Source::NwwsOi(s) => match Pin::new(s).poll_next(cx) {
                Poll::Ready(Some(Ok(m))) => Poll::Ready(Some(Ok(m.into()))),
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(SourceError(Box::new(e))))),
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Pending => Poll::Pending,
            },
        }
    }
}

#[derive(Debug)]
pub struct SourceError(Box<dyn std::error::Error + Send>);
impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for SourceError {}
