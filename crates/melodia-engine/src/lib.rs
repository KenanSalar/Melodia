//! The state machine, the queue, the action list and the backend that runs them: the top of the
//! audio stack, naming neither cpal nor the network, which the two tiers below it own.

pub mod player {
    pub mod engine;
}
