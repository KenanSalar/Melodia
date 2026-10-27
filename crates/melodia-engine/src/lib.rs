//! The state machine, the queue, the action list and the backend that runs them: the top of the
//! audio stack, naming neither cpal nor the network (`.claude/rules/audio-stack.md`).

pub mod player {
    pub mod engine;
}
