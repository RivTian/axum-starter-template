use axum::Json;
use axum::http::StatusCode;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct Envelope {
    status: &'static str,
    code: u16,
    description: &'static str,
}

impl Envelope {
    pub(crate) fn success() -> Json<Self> {
        Json(Self {
            status: "success",
            code: 200,
            description: "",
        })
    }
    pub(crate) fn error(status: StatusCode, description: &'static str) -> Json<Self> {
        Json(Self {
            status: "error",
            code: status.as_u16(),
            description,
        })
    }
}
