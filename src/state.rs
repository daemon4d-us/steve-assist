use std::env;
use std::sync::Arc;

use firestore::FirestoreDb;

use crate::services::db::{self, AgentProfile};
use crate::services::llm::{self, LlmProvider};

pub struct GoogleOAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub setup_secret: String,
}

pub struct AppState {
    pub llm: Arc<dyn LlmProvider>,
    pub elevenlabs_api_key: String,
    pub elevenlabs_voice_id: String,
    pub server_port: u16,
    pub firestore_db: FirestoreDb,
    pub memory_depth: u32,
    pub deepgram_api_key: Option<String>,
    pub allowed_emails: Vec<String>,
    pub agent_profile: AgentProfile,
    pub agent_profile_id: String,
    pub twilio_account_sid: String,
    pub twilio_auth_token: String,
    pub twilio_phone_number: String,
    pub server_host: String,
    pub google_oauth: Option<GoogleOAuthConfig>,
}

impl AppState {
    pub async fn new() -> Self {
        let gcp_project_id = env::var("GCP_PROJECT_ID").expect("GCP_PROJECT_ID must be set");

        let firestore_db = db::init(&gcp_project_id)
            .await
            .expect("Failed to initialize Firestore");

        tracing::info!("Firestore initialized for project {gcp_project_id}");

        let profile_id = env::var("AGENT_PROFILE_ID").unwrap_or_else(|_| "default".to_string());
        let agent_profile = db::get_agent_profile(&firestore_db, &profile_id)
            .await
            .expect("Failed to load agent profile from Firestore")
            .unwrap_or_else(|| {
                panic!("Agent profile '{profile_id}' not found in Firestore. Create it in the 'agent_profiles' collection.");
            });
        tracing::info!("Loaded agent profile: {profile_id}");

        let llm = llm::from_env();
        tracing::info!(
            "LLM provider: {} (default model {}, profile model {})",
            llm.name(),
            llm.default_model(),
            agent_profile.model
        );

        Self {
            llm,
            elevenlabs_api_key: env::var("ELEVENLABS_API_KEY")
                .expect("ELEVENLABS_API_KEY must be set"),
            elevenlabs_voice_id: env::var("ELEVENLABS_VOICE_ID")
                .expect("ELEVENLABS_VOICE_ID must be set"),
            server_port: env::var("SERVER_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .expect("SERVER_PORT must be a valid port number"),
            firestore_db,
            memory_depth: env::var("MEMORY_DEPTH")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .expect("MEMORY_DEPTH must be a valid number"),
            deepgram_api_key: env::var("DEEPGRAM_API_KEY").ok(),
            agent_profile,
            agent_profile_id: profile_id,
            twilio_account_sid: env::var("TWILIO_ACCOUNT_SID")
                .expect("TWILIO_ACCOUNT_SID must be set"),
            twilio_auth_token: env::var("TWILIO_AUTH_TOKEN")
                .expect("TWILIO_AUTH_TOKEN must be set"),
            twilio_phone_number: env::var("TWILIO_PHONE_NUMBER")
                .expect("TWILIO_PHONE_NUMBER must be set"),
            server_host: env::var("SERVER_HOST")
                .unwrap_or_else(|_| "steve.creativecaptains.com".to_string()),
            allowed_emails: env::var("ALLOWED_EMAILS")
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            google_oauth: load_google_oauth(),
        }
    }
}

fn load_google_oauth() -> Option<GoogleOAuthConfig> {
    let client_id = env::var("GOOGLE_OAUTH_CLIENT_ID").ok()?;
    let client_secret = env::var("GOOGLE_OAUTH_CLIENT_SECRET").ok()?;
    let redirect_uri = env::var("GOOGLE_OAUTH_REDIRECT_URI").ok()?;
    let setup_secret = env::var("GOOGLE_OAUTH_SETUP_SECRET").ok()?;
    if client_id.is_empty()
        || client_secret.is_empty()
        || redirect_uri.is_empty()
        || setup_secret.is_empty()
    {
        return None;
    }
    Some(GoogleOAuthConfig {
        client_id,
        client_secret,
        redirect_uri,
        setup_secret,
    })
}
