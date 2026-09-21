use crate::app::TelescopeApp;
use crate::app::messages::CharacterSync;
use crate::app::messages::MapSync;
use crate::app::messages::Message;
use crate::app::messages::Type;
use crate::app::messages::send_app_message;
use std::sync::Arc;
use std::thread;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::time::Duration;
use tokio::time::sleep;

impl TelescopeApp {
    #[tracing::instrument(skip(self))]
    pub fn start_watchdog(&mut self, character_id: Vec<usize>) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (sender, mut receiver) = mpsc::channel::<CharacterSync>(10);
        let app_sender = Arc::clone(&self.app_msg.0);
        let map_sender = Arc::clone(&self.map_msg.0);
        let mut t_esi = self.esi.clone();
        thread::spawn(move || {
            runtime.block_on(async {
                let _span = tracing::info_span!("spawned watchdog").entered();

                let mut character_ids = vec![];
                if let Err(t_error) = t_esi.update_spec().await {
                    let _ = send_app_message(
                        &app_sender,
                        Message::GenericNotification((
                            Type::Error,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            t_error.to_string(),
                        )),
                    )
                    .await;
                } else {
                    let _ = send_app_message(
                        &app_sender,
                        Message::GenericNotification((
                            Type::Info,
                            String::from("Telescope App"),
                            String::from("start_watchdog"),
                            String::from("Starting watchdog"),
                        )),
                    )
                    .await;
                }
                for char_id in character_id {
                    character_ids.push((char_id, 0))
                }
                while !character_ids.is_empty() {
                    if !t_esi.valid_token().await {
                        if let Err(t_error) = t_esi.refresh_token().await {
                            let _ = send_app_message(
                                &app_sender,
                                Message::GenericNotification((
                                    Type::Error,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    t_error.to_string(),
                                )),
                            )
                            .await;
                            return;
                        } else {
                            let _ = send_app_message(
                                &app_sender,
                                Message::GenericNotification((
                                    Type::Debug,
                                    String::from("Telescope App"),
                                    String::from("start_watchdog"),
                                    String::from("token refreshed successfully"),
                                )),
                            )
                            .await;
                        }
                    }
                    for item in &mut character_ids {
                        //PlayerDatabase
                        match t_esi.get_location(item.0.try_into().unwrap()).await {
                            Ok(new_location) => {
                                if item.1 != (new_location as usize) {
                                    item.1 = new_location as usize;
                                    let _ = send_app_message(
                                        &app_sender,
                                        Message::PlayerNewLocation((
                                            item.0.try_into().unwrap(),
                                            new_location,
                                        )),
                                    )
                                    .await;
                                    if let Err(t_error) =
                                        map_sender.send(MapSync::PlayerMoved((item.0, item.1)))
                                    {
                                        let _ = send_app_message(
                                            &app_sender,
                                            Message::GenericNotification((
                                                Type::Error,
                                                String::from("Telescope App"),
                                                String::from("start_watchdog"),
                                                t_error.to_string(),
                                            )),
                                        )
                                        .await;
                                    }
                                }
                            }
                            Err(t_error) => {
                                let _ = send_app_message(
                                    &app_sender,
                                    Message::GenericNotification((
                                        Type::Error,
                                        String::from("Telescope App"),
                                        String::from("start_watchdog - get_location - ")
                                            + item.0.to_string().as_str(),
                                        t_error.to_string(),
                                    )),
                                )
                                .await;
                                break;
                            }
                        }
                    }
                    sleep(Duration::new(5, 0)).await;
                    while let Ok(message) = receiver.try_recv() {
                        match message {
                            CharacterSync::Add(char_data) => character_ids.push((char_data, 0)),
                            CharacterSync::Remove(char_id) => {
                                for index in 0..character_ids.len() {
                                    if character_ids[index].0 == char_id {
                                        character_ids.remove(index);
                                        break;
                                    }
                                }
                                if character_ids.is_empty() {
                                    let _ = send_app_message(
                                        &app_sender,
                                        Message::GenericNotification((
                                            Type::Info,
                                            String::from("Telescope App"),
                                            String::from("start_watchdog"),
                                            String::from("Watchdog ended"),
                                        )),
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                    if let Err(TryRecvError::Disconnected) = receiver.try_recv() {
                        character_ids.clear();
                        let _ = send_app_message(
                            &app_sender,
                            Message::GenericNotification((
                                Type::Info,
                                String::from("Telescope App"),
                                String::from("start_watchdog"),
                                String::from("Watchdog ended"),
                            )),
                        )
                        .await;
                        break;
                    }
                    sleep(Duration::new(25, 0)).await;
                }
            });
        });

        self.char_msg = Some(Arc::new(sender));
    }
}
