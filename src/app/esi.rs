use rfesi::prelude::*;

fn create_esi() -> EsiResult<Esi> {
    // Create a new struct from the builder. These parameters
    // all come from your third-party app on the developers site.
    EsiBuilder::new()
        .user_agent("telescope/0.0.1")
        .client_id("e1c1d13678554397baab37e6eeade7a5 ")
        .client_secret("eESiu5D8f1y7VA2STH0B9S0Mbsai1TcdmOxNqIrk")
        .callback_url("http://localhost/login/")
        .build()
}

fn get_authorize_url(esi: &Esi) -> (String, String) {
    // Direct your user to the tuple's first item, a URL, and have a web service listening
    // at the callback URL that you specified in the EVE application. The second item is
    // the random state variable, which is up to you to check.
    esi.get_authorize_url().unwrap()
}

async fn authenticate_user(esi: &mut Esi, code: &str) -> EsiResult<()> {
    // The `code` value here comes from the URL parameters your service
    // is sent following a user's successful SSO.
    //
    // Note that most functions in this crate are async, so you'll need
    // to handle those appropriately.
    //
    // Additionally, this function requires a mutable reference to the
    // struct, as the instance will self-mutate with the additional information
    // from ESI (assuming a successful authorization).
    //
    // Once the instance has the auth information, you can use it to make
    // authenticated requests to ESI for the user.
    esi.authenticate(code).await?;
    Ok(())
}