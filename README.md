# `nwws-http`

HTTP transport for the [NOAA Weather Wire Service](https://www.weather.gov/nwws/).

The National Weather Service operates [several platforms](https://www.weather.gov/nwws/dissemination) for distributing
[text products](https://forecast.weather.gov/product_types.php?site=NWS). None of the platforms provide realtime access
to text products as they become available over the web. NWWS-OI is closest, but it a) requires signup and b) uses XMPP.
`nwws-http` bridges the gap between NWWS-OI and HTTP.

## Running a server

1. [Sign up](https://www.weather.gov/nwws/nwws_oi_request) for NWWS-OI credentials
2. Receive credentials from the NWS by email
3. Run the Docker image in this repository with `NWWS_OI_USERNAME` and `NWWS_OI_PASSWORD` environment variables

A public instance is running on a best-effort basis at [nwws-http.fly.dev](https://nwws-http.fly.dev).

## Protocol

`nwws-http` uses [Server-Sent Events](https://en.wikipedia.org/wiki/Server-sent_events) to transmit NWWS messages over
HTTP as they arrive. A typical exchange:

```http request
GET /stream HTTP/1.1
Accept: text/event-stream

HTTP/1.1 200 OK
Content-Type: text/event-stream; charset=utf-8
Cache-Control: private, no-cache

id:29116.17989
data:{"ttaaii":"WGUS44","cccc":"KMOB","awips_id":"FLWMOB","issue":"2022-02-05T23:15:00+00:00","id":"29116.17989","message":"\n817\nWGUS44 KMOB 052315\nFLWMOB\n\nBULLETIN - IMMEDIATE BROADCAST REQUESTED\nFlood Warning\n…"}

id:29116.17990
data:{"ttaaii":"NTXX99","cccc":"PHEB","awips_id":"TSTHEB","issue":"2022-02-05T23:15:00+00:00","id":"29116.17990","message":"\n818\nNTXX99 PHEB 052315\nTSTHEB\nredundant-side test from PTWC IRC\nRZRZRZRZRZRZRZRZRZRZRZRZRZRZ\nRZRZRZRZRZRZRZRZRZRZRZRZRZRZ\n"}

```

The `/stream` endpoint supports filtering by `?cccc=…`, `?ttaaii=…`, and/or `?awips_id=…`.

## Rust crate

This repository contains a Rust crate, which can be used with `--feature client` or `--feature server`. The server
application is `--bin nwws-http-server`.
