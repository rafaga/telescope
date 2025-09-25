use hyper::{Method, StatusCode};
use tokio::runtime::Builder;
use tokio::sync::mpsc::Sender;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::Service;
use hyper::{Request, Response, body::Incoming as IncomingBody};

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

static CONFIRM: &[u8] = b"<html><head><title>Telescope login</title><style>body{font-family: monospace;background-color: gray;color: whitesmoke;}</style></head><body><h1>Telescope</h1><p>Logged in!, now you can close this window safely.</p></body></html>";
static NOT_VALID: &[u8] = b"Invalid Request";

#[derive(Debug, Clone)]
pub struct AuthService2 {
    pub tx: Arc<Sender<(String, String)>>,
}

impl Service<Request<IncomingBody>> for AuthService2 {
    type Response = Response<Full<Bytes>>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn call(&self, req: Request<IncomingBody>) -> Self::Future {
        #[cfg(feature = "puffin")]
        puffin::profile_function!();

        let res = match (req.method(), req.uri().path()) {
            (&Method::GET, "/login") => {
                let pnq = req.uri().path_and_query();
                if let Some(params) = pnq.unwrap().query() {
                    let mut message: (String, String) = (String::new(), String::new());
                    let parameters = params.split('&').collect::<Vec<&str>>();
                    for param in parameters {
                        let p = param.split('=').collect::<Vec<&str>>();
                        match p[0] {
                            "code" => {
                                message.0 = p[1].to_string();
                            }
                            "state" => {
                                message.1 = p[1].to_string();
                            }
                            _ => (),
                        }
                    }
                    if !message.0.is_empty() && !message.1.is_empty() {
                        let rt = Builder::new_current_thread().enable_all().build().unwrap();
                        let atx = Arc::clone(&self.tx);
                        std::thread::spawn(move || {
                            rt.block_on(async {
                                #[cfg(feature = "puffin")]
                                puffin::profile_scope!("http service request response");

                                let _res = atx.send(message).await;
                            });
                        });
                        Ok(Response::builder()
                            .status(StatusCode::OK)
                            .body(Full::new(Bytes::from_static(CONFIRM)))
                            .unwrap())
                    } else {
                        Ok(Response::builder()
                            .status(StatusCode::UNPROCESSABLE_ENTITY)
                            .body(Full::new(Bytes::from_static(NOT_VALID)))
                            .unwrap())
                    }
                } else {
                    Ok(Response::builder()
                        .status(StatusCode::UNPROCESSABLE_ENTITY)
                        .body(Full::new(Bytes::from_static(NOT_VALID)))
                        .unwrap())
                }
            }
            _ => Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from_static(NOT_VALID)))
                .unwrap()),
        };
        Box::pin(async { res })
    }
}
