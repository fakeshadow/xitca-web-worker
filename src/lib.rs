mod utils;

use std::{cell::RefCell, rc::Rc};

use worker::*;
use xitca_http::{
    http,
    util::service::{
        route::{get, RouteError},
        router::{Router, RouterError},
    },
};
use xitca_service::{fn_service, object, Service, ServiceExt};
use xitca_unsafe_collection::fake_send_sync::{FakeSend, FakeSync};

// thread local for storing router service.
thread_local! {
    static R: RefCell<Option<RouterService>> = RefCell::new(None);
}

// type alias to reduce type complexity.
type RouterService =
    Rc<dyn object::ServiceObject<http::Request<()>, Response = Response, Error = Error>>;

fn log_request(req: &Request) {
    console_log!(
        "{} - [{}], located at: {:?}, within: {}",
        Date::now().to_string(),
        req.path(),
        req.cf().coordinates().unwrap_or_default(),
        req.cf().region().unwrap_or("unknown region".into())
    );
}

#[event(fetch)]
pub async fn main(req: Request, env: Env, _: Context) -> Result<Response> {
    log_request(&req);

    // Optionally, get more helpful error messages written to the console in the case of a panic.
    utils::set_panic_hook();

    // initialize router once.
    if R.with(|r| r.borrow().is_none()) {
        let service = Router::new()
            .insert("/", get(fn_service(index)))
            .insert("/worker-version", get(fn_service(version)))
            .enclosed_fn(error_handler)
            .call(())
            .await
            .unwrap();

        R.with(|r| *r.borrow_mut() = Some(Rc::new(service)));
    }

    // clone router service to async context.
    let router = R.with(|r| r.borrow().as_ref().cloned().unwrap());

    // convert worker request to http request.
    let mut http_req = http::Request::new(());

    // naive url to uri conversion. only request path is covered.
    *http_req.uri_mut() = req.url()
        .ok()
        .and_then(|url| std::str::FromStr::from_str(url.path()).ok())
        .unwrap_or_else(|| http::Uri::from_static("/not_found"));

    *http_req.method_mut() = match req.method() {
        Method::Get => http::Method::GET,
        Method::Post => http::Method::POST,
        _ => http::Method::DELETE, // not interested methods.
    };

    // potential body conversion if include middleware wants body type.

    // store Env and Request in type map to use later.
    http_req
        .extensions_mut()
        .insert(FakeSync::new(FakeSend::new(env)));
    http_req
        .extensions_mut()
        .insert(FakeSync::new(FakeSend::new(req)));

    // call router service
    router.call(http_req).await
}

// error handler
async fn error_handler<S>(service: &S, req: http::Request<()>) -> Result<Response>
where
    S: Service<http::Request<()>, Response = Response, Error = RouterError<RouteError<Error>>>,
{
    match service.call(req).await {
        Ok(res) => Ok(res),
        Err(RouterError::First(_)) => Response::error("NotFound", 404),
        Err(RouterError::Second(RouteError::First(_))) => Response::error("MethodNotAllowed", 405),
        Err(RouterError::Second(RouteError::Second(e))) => Err(e)
    }
}

async fn index(_: http::Request<()>) -> Result<Response> {
    Response::ok("Hello from Workers!")
}

async fn version(mut req: http::Request<()>) -> Result<Response> {
    let env = req
        .extensions_mut()
        .remove::<FakeSync<FakeSend<Env>>>()
        .unwrap()
        .into_inner()
        .into_inner();
    let version = env.var("WORKERS_RS_VERSION")?.to_string();
    Response::ok(version)
}
