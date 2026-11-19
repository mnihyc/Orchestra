//! Exact operational values are decoded at the database boundary, before their typed use.
use orch_types::{Json, storable};

#[derive(sqlx::Type)]
#[sqlx(transparent, no_pg_array)]
pub struct Text(String);
impl TryFrom<Text> for String {
    type Error = storable::InvalidEncoding;
    fn try_from(value: Text) -> Result<Self, Self::Error> {
        storable::decode_key(&value.0)
    }
}

#[derive(sqlx::Type)]
#[sqlx(transparent, no_pg_array)]
pub struct Stream(String);
impl TryFrom<Stream> for String {
    type Error = storable::InvalidEncoding;
    fn try_from(value: Stream) -> Result<Self, Self::Error> {
        storable::decode_stream(&value.0)
    }
}

#[derive(sqlx::Type)]
#[sqlx(transparent, no_pg_array)]
pub struct OptionalText(Option<String>);
impl TryFrom<OptionalText> for Option<String> {
    type Error = storable::InvalidEncoding;
    fn try_from(value: OptionalText) -> Result<Self, Self::Error> {
        value.0.as_deref().map(storable::decode_key).transpose()
    }
}

#[derive(sqlx::Type)]
#[sqlx(transparent, no_pg_array)]
pub struct ExactJson(Json);
impl TryFrom<ExactJson> for Json {
    type Error = storable::InvalidEncoding;
    fn try_from(value: ExactJson) -> Result<Self, Self::Error> {
        storable::decode_exact(value.0)
    }
}

#[derive(sqlx::Type)]
#[sqlx(transparent, no_pg_array)]
pub struct OptionalJson(Option<Json>);
impl TryFrom<OptionalJson> for Option<Json> {
    type Error = storable::InvalidEncoding;
    fn try_from(value: OptionalJson) -> Result<Self, Self::Error> {
        value.0.map(storable::decode_exact).transpose()
    }
}
