pub mod general;
pub mod music;
pub mod report;

use crate::{Data, Error};

pub fn all() -> Vec<poise::Command<Data, Error>> {
    vec![
        music::play(),
        music::skip(),
        music::stop(),
        music::pause(),
        music::resume(),
        music::seek(),
        music::forward(),
        music::rewind(),
        music::volume(),
        music::loop_mode(),
        music::shuffle(),
        music::clear_queue(),
        music::remove(),
        music::move_track(),
        music::queue(),
        music::nowplaying(),
        general::ping(),
        general::join(),
        general::leave(),
        general::setprefix(),
        general::uptime(),
        general::help_cmd(),
        report::report(),
    ]
}
