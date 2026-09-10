//! Steve: an AI phone assistant. The library exposes the server's building
//! blocks so the `steve-assist` server binary and the `steve-e2e` test harness
//! share one implementation of the Twilio protocol, speech, LLM and storage code.

pub mod handlers;
pub mod services;
pub mod state;
pub mod twilio;
