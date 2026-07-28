//! Yew progressive client.
//!
//! The web server's small JavaScript client remains the production bootstrap and
//! the observer has a no-Wasm fallback. This crate provides the Rust/Wasm client
//! boundary and can be built with Trunk without changing the game API.

#[cfg(target_arch = "wasm32")]
mod browser {
    use gloo_net::http::Request;
    use mille_protocol::View;
    use wasm_bindgen::prelude::{JsValue, wasm_bindgen};
    use wasm_bindgen_futures::spawn_local;
    use yew::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        #[wasm_bindgen(js_name = "URL")]
        type BrowserUrl;

        #[wasm_bindgen(catch, constructor, js_class = "URL")]
        fn new_with_base(url: &str, base: &str) -> Result<BrowserUrl, JsValue>;

        #[wasm_bindgen(method, getter, js_class = "URL", js_name = "href")]
        fn href(this: &BrowserUrl) -> String;
    }

    pub enum Message {
        Loaded(Box<View>),
        LoadFailed(String),
        Refresh,
    }

    pub struct App {
        view: Option<View>,
        error: Option<String>,
    }

    impl Component for App {
        type Message = Message;
        type Properties = ();

        fn create(context: &Context<Self>) -> Self {
            context.link().send_message(Message::Refresh);
            Self {
                view: None,
                error: None,
            }
        }

        fn update(&mut self, context: &Context<Self>, message: Self::Message) -> bool {
            match message {
                Message::Refresh => {
                    let link = context.link().clone();
                    spawn_local(async move {
                        let message = match load_public_view().await {
                            Ok(view) => Message::Loaded(Box::new(view)),
                            Err(error) => Message::LoadFailed(error),
                        };
                        link.send_message(message);
                    });
                    false
                }
                Message::Loaded(view) => {
                    self.view = Some(*view);
                    self.error = None;
                    true
                }
                Message::LoadFailed(error) => {
                    self.error = Some(error);
                    true
                }
            }
        }

        fn view(&self, context: &Context<Self>) -> Html {
            let refresh = context.link().callback(|_| Message::Refresh);
            let rendered = match (&self.view, &self.error) {
                (Some(View::Observer(public)), _) => html! {
                    <>
                        <h1>{&public.room_name}</h1>
                        <p>{format!("{:?}", public.lifecycle)}</p>
                        <div class="scoreboard">
                            {for public.players.iter().map(|player| html! {
                                <div class="player-tile">
                                    <b>{&player.name}</b>
                                    <br/>{format!("{} · {} cards", player.score, player.card_count)}
                                </div>
                            })}
                        </div>
                    </>
                },
                (_, Some(error)) => html! { <p>{error}</p> },
                _ => html! { <p>{"Loading…"}</p> },
            };
            html! {
                <main>
                    {rendered}
                    <button onclick={refresh}>{"Refresh"}</button>
                </main>
            }
        }
    }

    async fn load_public_view() -> Result<View, String> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let body = document.body().ok_or("no body")?;
        let configured_api = body.dataset().get("gameApi").unwrap_or_default();
        let api_reference = if configured_api.is_empty() {
            "game-api"
        } else {
            &configured_api
        };
        let base_uri = document
            .base_uri()
            .map_err(|_| "cannot read document base URI")?
            .ok_or("missing document base URI")?;
        let api = BrowserUrl::new_with_base(api_reference, &base_uri)
            .map_err(|error| {
                error
                    .as_string()
                    .unwrap_or_else(|| "invalid game API URL".to_owned())
            })?
            .href()
            .trim_end_matches('/')
            .to_owned();
        let room = body.dataset().get("room").ok_or("missing room")?;
        Request::get(&format!("{api}/api/rooms/{room}/view?role=observer"))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .json::<View>()
            .await
            .map_err(|error| error.to_string())
    }

    #[wasm_bindgen::prelude::wasm_bindgen(start)]
    pub fn start() {
        yew::Renderer::<App>::new().render();
    }
}

#[must_use]
pub const fn client_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
