//! Compile the frontend into the host binary; never read assets from the CWD.
use actix_web::{http::header, web, HttpResponse};
use include_dir::{include_dir, Dir};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../frontend/dist");

pub fn configure(config: &mut web::ServiceConfig) {
    config
        .route("/", web::get().to(index))
        .route("/favicon.svg", web::get().to(favicon))
        .route("/assets/{path:.*}", web::get().to(asset));
}

async fn index() -> HttpResponse {
    response("index.html", "text/html; charset=utf-8", false)
}

async fn favicon() -> HttpResponse {
    response("favicon.svg", "image/svg+xml", false)
}

async fn asset(path: web::Path<String>) -> HttpResponse {
    // Only an exact embedded key can be served; there is no filesystem fallback.
    let path = format!("assets/{}", path.into_inner());
    let content_type = match path.rsplit('.').next() {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => return HttpResponse::NotFound().finish(),
    };
    response(&path, content_type, true)
}

fn response(path: &str, content_type: &'static str, immutable: bool) -> HttpResponse {
    let Some(file) = ASSETS.get_file(path) else {
        return HttpResponse::NotFound().finish();
    };
    HttpResponse::Ok()
        .content_type(content_type)
        .insert_header((header::CACHE_CONTROL, if immutable { "public, max-age=31536000, immutable" } else { "no-cache" }))
        .insert_header((header::X_CONTENT_TYPE_OPTIONS, "nosniff"))
        .insert_header((header::REFERRER_POLICY, "no-referrer"))
        // Radix positions portals and installs modal scroll-lock styles inline.
        // Script execution remains same-origin only; no unsafe-eval or inline JS.
        .insert_header((header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'"))
        .body(file.contents())
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{test, App};

    #[actix_web::test]
    async fn serves_embedded_shell_and_assets_without_a_filesystem_fallback() {
        let app = test::init_service(App::new().configure(configure)).await;
        let response =
            test::call_service(&app, test::TestRequest::get().uri("/").to_request()).await;
        assert!(response.status().is_success());
        assert!(response
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY));
        let html = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
        assert!(html.contains("Cloudlet"));
        let script = html
            .split("src=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let response =
            test::call_service(&app, test::TestRequest::get().uri(script).to_request()).await;
        assert!(response.status().is_success());
        for path in [
            "/assets/missing.js",
            "/assets/../../Cargo.toml",
            "/api/missing",
        ] {
            let response =
                test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
            assert_eq!(response.status(), 404);
        }
    }
}
