use serde::Deserialize;

// WARN: The profile id in the json response
// has format "11111111222233334444555555555555"
// which needs to be changed into format "11111111-2222-3333-4444-555555555555"
// before sending it back to the client.

#[derive(Deserialize)]
pub struct Property<'r> {
    pub name: &'r str,
    pub value: &'r str,
    pub signature: &'r str,
}

#[derive(Deserialize)]
pub struct Response<'r> {
    pub id: &'r str,
    pub name: &'r str,
    pub properties: Vec<Property<'r>>,
}
