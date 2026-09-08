use axum::Router;
use dioxus::prelude::*;
use dioxus_liveview::LiveviewRouter;

fn app() -> Element {
    let mut num = use_signal(|| 0);

    rsx! {
        div {
            "hello axum! {num}"
            button { onclick: move |_| num += 1, "Increment" }
            button {
                onclick: move |_| {
                    let contents = format!("Current count: {}\n", num());
                    let file = dioxus_fullstack::FileStream::from_raw(
                        "count.txt".to_string(),
                        Some(contents.len() as u64),
                        "text/plain; charset=utf-8".to_string(),
                        axum::body::Body::from(contents).into_data_stream(),
                    );
                    if let Err(error) = dioxus_liveview::download_file(file) {
                        tracing::error!(%error, "Failed to queue count download");
                    }
                },
                "Download count"
            }
        }
    }
}

#[tokio::main]
async fn main() {
    dioxus::logger::initialize_default();

    let addr: std::net::SocketAddr = ([127, 0, 0, 1], 3030).into();

    let app = Router::new().with_app("/", app);

    println!("Listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();
}
