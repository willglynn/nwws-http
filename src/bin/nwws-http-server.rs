use hyper::http::{HeaderValue, Response};
use hyper::{header, Body, Request, StatusCode};
use std::convert::Infallible;
use std::fmt::Debug;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    env_logger::builder()
        .filter(None, log::LevelFilter::Info)
        .filter_module("nwws_oi", log::LevelFilter::Info)
        .filter_module("nwws_http", log::LevelFilter::Info)
        .parse_default_env()
        .init();

    let source = nwws_http::Source::from_env();

    let addr: SocketAddr = std::env::var("LISTEN")
        .ok()
        .map(|spec| spec.parse().expect("LISTEN must be valid if set"))
        .or_else(|| {
            std::env::var("PORT").ok().map(|port| {
                let port = port.parse::<u16>().expect("PORT must be valid if set");
                SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into()
            })
        })
        .unwrap_or(SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, 8080, 0, 0).into());

    let allow_all_origins = std::env::var_os("ALLOW_ALL_ORIGINS")
        .filter(|v| v != "")
        .is_some();

    let endpoint = nwws_http::server::Server::new(source);
    let context = Arc::new(Context {
        allow_all_origins,
        stream: endpoint,
    });

    let make_service =
        hyper::service::make_service_fn(move |socket: &hyper::server::conn::AddrStream| {
            let context = context.clone();
            let addr = socket.remote_addr();
            async move {
                Ok::<_, Infallible>(hyper::service::service_fn(move |req| {
                    let context = context.clone();
                    let addr = addr.clone();
                    async move { context.serve(addr, req).await }
                }))
            }
        });

    let server = hyper::server::Server::bind(&addr).serve(make_service);
    log::info!("listening for HTTP requests on {}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

#[derive(Debug)]
struct Context {
    stream: nwws_http::server::Server,
    allow_all_origins: bool,
}

impl Context {
    async fn serve(
        &self,
        remote_addr: SocketAddr,
        request: Request<Body>,
    ) -> hyper::http::Result<Response<Body>> {
        log::info!(
            "request from {}: {} {}",
            remote_addr,
            request.method(),
            request.uri().path()
        );

        match request.uri().path() {
            "/" => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(include_str!("index.html").into()),
            "/stream" => self.stream.stream(request).await,
            _ => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain")
                .body("not found".into()),
        }
        .map(|mut response| {
            if self.allow_all_origins {
                response.headers_mut().insert(
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_static("*"),
                );
            }
            response
        })
    }
}
