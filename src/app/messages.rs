use sde::objects::SystemPoint;

pub enum Message{
    GenericUpdate(String),
    Error(String),
    Processed2dMatrix(Vec<SystemPoint>),
}

