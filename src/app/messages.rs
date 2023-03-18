use sde::objects::SystemPoint;

pub enum Message{
    Processed2dMatrix(Vec<SystemPoint>),
}

