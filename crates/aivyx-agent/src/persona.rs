//! Structured personality configuration for agents.
//!
//! Replaces freeform `soul` strings with typed, tunable personality
//! dimensions that compose into natural-language system prompts via
//! [`Persona::generate_soul()`].

use serde::{Deserialize, Serialize};

// ── Default value functions for serde ──────────────────────────────

fn default_formality() -> f32 {
    0.5
}
fn default_verbosity() -> f32 {
    0.5
}
fn default_warmth() -> f32 {
    0.5
}
fn default_humor() -> f32 {
    0.2
}
fn default_confidence() -> f32 {
    0.6
}
fn default_curiosity() -> f32 {
    0.5
}
fn default_true() -> bool {
    true
}

/// Structured personality for an agent.
///
/// A `Persona` holds 6 personality dimensions (each `f32` in `[0.0, 1.0]`),
/// voice/style fields, and communication toggles. Call [`generate_soul()`](Persona::generate_soul)
/// to produce a natural-language system prompt suitable for an LLM.
///
/// # Example
///
/// ```
/// use aivyx_agent::Persona;
///
/// let persona = Persona::for_role("coder").unwrap();
/// let soul = persona.generate_soul("coder");
/// assert!(soul.contains("coder"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Persona {
    // ── Personality dimensions (0.0..=1.0) ─────────────────────────
    /// Formality: 0.0 = casual, 1.0 = formal.
    #[serde(default = "default_formality")]
    pub formality: f32,

    /// Verbosity: 0.0 = terse, 1.0 = very detailed.
    #[serde(default = "default_verbosity")]
    pub verbosity: f32,

    /// Warmth: 0.0 = neutral/professional, 1.0 = warm and friendly.
    #[serde(default = "default_warmth")]
    pub warmth: f32,

    /// Humor: 0.0 = no humor, 1.0 = frequently humorous.
    #[serde(default = "default_humor")]
    pub humor: f32,

    /// Confidence: 0.0 = hedging, 1.0 = assertive.
    #[serde(default = "default_confidence")]
    pub confidence: f32,

    /// Curiosity: 0.0 = just answers, 1.0 = probes and explores.
    #[serde(default = "default_curiosity")]
    pub curiosity: f32,

    // ── Voice & style ──────────────────────────────────────────────
    /// Tone descriptor (e.g. "professional", "mentoring", "friendly").
    #[serde(default)]
    pub tone: Option<String>,

    /// Language complexity (e.g. "simple", "technical", "academic").
    #[serde(default)]
    pub language_level: Option<String>,

    /// Code style notes (e.g. "idiomatic Rust", "functional").
    #[serde(default)]
    pub code_style: Option<String>,

    /// Error reporting style (e.g. "gentle", "direct", "detailed").
    #[serde(default)]
    pub error_style: Option<String>,

    /// Greeting template. `{name}` is replaced with the user's name if known.
    #[serde(default)]
    pub greeting: Option<String>,

    // ── Communication toggles ──────────────────────────────────────
    /// Use emoji in responses.
    #[serde(default)]
    pub uses_emoji: bool,

    /// Use analogies and metaphors to explain concepts.
    #[serde(default = "default_true")]
    pub uses_analogies: bool,

    /// Proactively ask follow-up questions.
    #[serde(default = "default_true")]
    pub asks_followups: bool,

    /// Explicitly admit uncertainty rather than guessing.
    #[serde(default = "default_true")]
    pub admits_uncertainty: bool,
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            formality: default_formality(),
            verbosity: default_verbosity(),
            warmth: default_warmth(),
            humor: default_humor(),
            confidence: default_confidence(),
            curiosity: default_curiosity(),
            tone: None,
            language_level: None,
            code_style: None,
            error_style: None,
            greeting: None,
            uses_emoji: false,
            uses_analogies: default_true(),
            asks_followups: default_true(),
            admits_uncertainty: default_true(),
        }
    }
}

impl Persona {
    /// Clamp all dimension values to `[0.0, 1.0]`.
    pub fn normalize(&mut self) {
        self.formality = self.formality.clamp(0.0, 1.0);
        self.verbosity = self.verbosity.clamp(0.0, 1.0);
        self.warmth = self.warmth.clamp(0.0, 1.0);
        self.humor = self.humor.clamp(0.0, 1.0);
        self.confidence = self.confidence.clamp(0.0, 1.0);
        self.curiosity = self.curiosity.clamp(0.0, 1.0);
    }

    /// All available preset role names.
    pub fn preset_names() -> &'static [&'static str] {
        &[
            // General presets
            "assistant",
            "coder",
            "researcher",
            "writer",
            "ops",
            // Nonagon team presets
            "coordinator",
            "nonagon-researcher",
            "analyst",
            "nonagon-coder",
            "reviewer",
            "nonagon-writer",
            "planner",
            "guardian",
            "executor",
        ]
    }

    /// Create a preset persona for a known role.
    ///
    /// Returns `None` if the role has no preset.
    pub fn for_role(role: &str) -> Option<Self> {
        match role {
            "assistant" => Some(Self::assistant()),
            "coder" => Some(Self::coder()),
            "researcher" => Some(Self::researcher()),
            "writer" => Some(Self::writer()),
            "ops" => Some(Self::ops()),
            // Nonagon team presets
            "coordinator" => Some(Self::nonagon_coordinator()),
            "nonagon-researcher" => Some(Self::nonagon_researcher()),
            "analyst" => Some(Self::nonagon_analyst()),
            "nonagon-coder" => Some(Self::nonagon_coder()),
            "reviewer" => Some(Self::nonagon_reviewer()),
            "nonagon-writer" => Some(Self::nonagon_writer()),
            "planner" => Some(Self::nonagon_planner()),
            "guardian" => Some(Self::nonagon_guardian()),
            "executor" => Some(Self::nonagon_executor()),
            _ => None,
        }
    }

    /// Generate a natural-language system prompt from this persona's fields.
    ///
    /// The result is suitable for use as the `soul` / system prompt of an
    /// agent. The `role` parameter is included in the opening sentence.
    ///
    /// When a custom `soul` string is also present on the profile, prefer
    /// [`generate_guidelines()`](Self::generate_guidelines) instead — it
    /// produces only the behavioral rules without the role introduction,
    /// allowing the custom soul to provide the agent's identity.
    pub fn generate_soul(&self, role: &str) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("You are an AI {role}."));
        parts.extend(self.build_behavioral_rules());
        parts.join(" ")
    }

    /// Generate behavioral guidelines without a role introduction.
    ///
    /// Use this when the agent has a custom `soul` string that already
    /// defines its identity. The guidelines are appended after the soul
    /// to layer structured personality rules on top of the user-crafted
    /// identity prompt.
    pub fn generate_guidelines(&self) -> String {
        self.build_behavioral_rules().join(" ")
    }

    /// Build the common set of behavioral rules from persona dimensions.
    fn build_behavioral_rules(&self) -> Vec<String> {
        let mut parts: Vec<String> = Vec::new();

        // ── Formality ──────────────────────────────────────────────
        if self.formality < 0.3 {
            parts.push("Use casual, conversational language.".into());
        } else if self.formality > 0.7 {
            parts.push("Use formal, professional language.".into());
        }

        // ── Verbosity ──────────────────────────────────────────────
        if self.verbosity < 0.3 {
            parts.push("Be concise and terse — favor brevity over detail.".into());
        } else if self.verbosity > 0.7 {
            parts.push("Be thorough and detailed in your explanations.".into());
        } else {
            parts.push("Balance conciseness with enough detail to be helpful.".into());
        }

        // ── Warmth ─────────────────────────────────────────────────
        if self.warmth > 0.7 {
            parts.push("Be warm, encouraging, and personable.".into());
        } else if self.warmth < 0.3 {
            parts.push("Maintain a neutral, matter-of-fact tone.".into());
        }

        // ── Humor ──────────────────────────────────────────────────
        if self.humor > 0.5 {
            parts.push("Use appropriate humor when it helps engage or clarify.".into());
        } else if self.humor < 0.2 {
            parts.push("Keep responses straightforward — avoid humor.".into());
        }

        // ── Confidence ─────────────────────────────────────────────
        if self.confidence > 0.7 {
            parts.push("State your conclusions directly and confidently.".into());
        } else if self.confidence < 0.3 {
            parts.push("Qualify uncertain statements and present alternatives.".into());
        }

        // ── Curiosity ──────────────────────────────────────────────
        if self.curiosity > 0.7 {
            parts.push(
                "Proactively explore the problem space — ask clarifying questions and surface related considerations.".into(),
            );
        } else if self.curiosity < 0.3 {
            parts.push("Focus on answering the question at hand without probing further.".into());
        }

        // ── Voice & style fields ───────────────────────────────────
        if let Some(ref tone) = self.tone {
            parts.push(format!("Your overall tone is {tone}."));
        }
        if let Some(ref level) = self.language_level {
            parts.push(format!("Adjust language complexity to a {level} level."));
        }
        if let Some(ref cs) = self.code_style {
            parts.push(format!("When writing code: {cs}."));
        }
        if let Some(ref es) = self.error_style {
            parts.push(format!("When reporting errors or issues: {es}."));
        }

        // ── Communication toggles ──────────────────────────────────
        if self.uses_emoji {
            parts.push("Use emoji when appropriate to add warmth or emphasis.".into());
        } else {
            parts.push("Do not use emoji in responses.".into());
        }

        if self.uses_analogies {
            parts.push("Use analogies and metaphors when they help explain complex ideas.".into());
        }

        if self.asks_followups {
            parts.push("Ask follow-up questions when the request is ambiguous.".into());
        }

        if self.admits_uncertainty {
            parts.push(
                "Admit uncertainty honestly — never fabricate facts or pretend to know something you don't.".into(),
            );
        }

        // ── Greeting ──────────────────────────────────────────────
        if let Some(ref greeting) = self.greeting {
            parts.push(format!("When greeting the user: {greeting}."));
        }

        parts
    }

    // ── Preset constructors ────────────────────────────────────────

    fn assistant() -> Self {
        Self {
            formality: 0.4,
            verbosity: 0.4,
            warmth: 0.7,
            humor: 0.3,
            confidence: 0.6,
            curiosity: 0.7,
            tone: Some("friendly and proactive".into()),
            language_level: None,
            code_style: None,
            error_style: Some("gentle and constructive".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: true,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    fn coder() -> Self {
        Self {
            formality: 0.6,
            verbosity: 0.5,
            warmth: 0.3,
            humor: 0.1,
            confidence: 0.8,
            curiosity: 0.4,
            tone: Some("technical and precise".into()),
            language_level: Some("technical".into()),
            code_style: Some(
                "clean, idiomatic, well-tested — use the type system to prevent bugs".into(),
            ),
            error_style: Some("direct with root-cause analysis".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    fn researcher() -> Self {
        Self {
            formality: 0.7,
            verbosity: 0.7,
            warmth: 0.4,
            humor: 0.1,
            confidence: 0.5,
            curiosity: 0.8,
            tone: Some("methodical and thorough".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("detailed with citations".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: true,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    fn writer() -> Self {
        Self {
            formality: 0.5,
            verbosity: 0.6,
            warmth: 0.6,
            humor: 0.2,
            confidence: 0.7,
            curiosity: 0.3,
            tone: Some("clear and engaging".into()),
            language_level: None,
            code_style: None,
            error_style: Some("constructive editing feedback".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: true,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    fn ops() -> Self {
        Self {
            formality: 0.6,
            verbosity: 0.4,
            warmth: 0.3,
            humor: 0.1,
            confidence: 0.8,
            curiosity: 0.3,
            tone: Some("reliable and security-conscious".into()),
            language_level: Some("technical".into()),
            code_style: Some("defensive scripting — always check exit codes".into()),
            error_style: Some("direct with remediation steps".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    // ── Nonagon team presets ────────────────────────────────────────

    /// Coordinator: orchestrates, delegates, synthesizes. High confidence,
    /// moderate formality, concise but clear.
    fn nonagon_coordinator() -> Self {
        Self {
            formality: 0.6,
            verbosity: 0.5,
            warmth: 0.4,
            humor: 0.1,
            confidence: 0.9,
            curiosity: 0.6,
            tone: Some("authoritative and decisive".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("concise with clear next steps".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    /// Researcher: meticulous, evidence-based, always cites confidence.
    fn nonagon_researcher() -> Self {
        Self {
            formality: 0.7,
            verbosity: 0.8,
            warmth: 0.3,
            humor: 0.0,
            confidence: 0.4,
            curiosity: 0.9,
            tone: Some("meticulous and evidence-based".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("detailed with confidence levels".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: true,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    /// Analyst: data-driven, pattern-focused, structured reporting.
    fn nonagon_analyst() -> Self {
        Self {
            formality: 0.7,
            verbosity: 0.6,
            warmth: 0.2,
            humor: 0.0,
            confidence: 0.7,
            curiosity: 0.6,
            tone: Some("analytical and structured".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("clear with supporting data".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    /// Coder (Nonagon): precise, test-driven, follows conventions.
    fn nonagon_coder() -> Self {
        Self {
            formality: 0.5,
            verbosity: 0.4,
            warmth: 0.2,
            humor: 0.0,
            confidence: 0.8,
            curiosity: 0.3,
            tone: Some("precise and solution-oriented".into()),
            language_level: Some("technical".into()),
            code_style: Some("idiomatic, tested, follows project conventions".into()),
            error_style: Some("direct with stack traces and root-cause".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    /// Reviewer: critical eye, security-aware, thorough.
    fn nonagon_reviewer() -> Self {
        Self {
            formality: 0.7,
            verbosity: 0.6,
            warmth: 0.2,
            humor: 0.0,
            confidence: 0.8,
            curiosity: 0.5,
            tone: Some("critical and constructive".into()),
            language_level: Some("technical".into()),
            code_style: Some("strict adherence to style guide and best practices".into()),
            error_style: Some("specific with suggested fixes".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    /// Writer (Nonagon): clear, audience-aware, well-structured.
    fn nonagon_writer() -> Self {
        Self {
            formality: 0.5,
            verbosity: 0.7,
            warmth: 0.5,
            humor: 0.1,
            confidence: 0.7,
            curiosity: 0.3,
            tone: Some("clear and audience-aware".into()),
            language_level: None,
            code_style: None,
            error_style: Some("constructive editing suggestions".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: true,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    /// Planner: structured thinking, risk-aware, dependency-oriented.
    fn nonagon_planner() -> Self {
        Self {
            formality: 0.6,
            verbosity: 0.7,
            warmth: 0.3,
            humor: 0.0,
            confidence: 0.7,
            curiosity: 0.6,
            tone: Some("systematic and risk-aware".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("flags risks with mitigation options".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: true,
            admits_uncertainty: true,
        }
    }

    /// Guardian: cautious, security-first, thorough auditing.
    fn nonagon_guardian() -> Self {
        Self {
            formality: 0.8,
            verbosity: 0.6,
            warmth: 0.1,
            humor: 0.0,
            confidence: 0.7,
            curiosity: 0.4,
            tone: Some("vigilant and security-focused".into()),
            language_level: Some("technical".into()),
            code_style: None,
            error_style: Some("severity-rated with remediation steps".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }

    /// Executor: minimal talk, maximum action, careful reporting.
    fn nonagon_executor() -> Self {
        Self {
            formality: 0.5,
            verbosity: 0.2,
            warmth: 0.1,
            humor: 0.0,
            confidence: 0.8,
            curiosity: 0.1,
            tone: Some("terse and action-oriented".into()),
            language_level: Some("technical".into()),
            code_style: Some("defensive scripting — always check exit codes".into()),
            error_style: Some("exact error output with exit codes".into()),
            greeting: None,
            uses_emoji: false,
            uses_analogies: false,
            asks_followups: false,
            admits_uncertainty: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimension_clamping() {
        let mut p = Persona {
            formality: -0.5,
            verbosity: 1.5,
            warmth: 0.5,
            humor: 2.0,
            confidence: -1.0,
            curiosity: 0.5,
            ..Default::default()
        };
        p.normalize();
        assert_eq!(p.formality, 0.0);
        assert_eq!(p.verbosity, 1.0);
        assert_eq!(p.warmth, 0.5);
        assert_eq!(p.humor, 1.0);
        assert_eq!(p.confidence, 0.0);
        assert_eq!(p.curiosity, 0.5);
    }

    #[test]
    fn generate_soul_includes_role() {
        let p = Persona::default();
        let soul = p.generate_soul("assistant");
        assert!(soul.contains("AI assistant"));
    }

    #[test]
    fn generate_soul_formality_extremes() {
        let mut casual = Persona::default();
        casual.formality = 0.0;
        assert!(casual.generate_soul("helper").contains("casual"));

        let mut formal = Persona::default();
        formal.formality = 1.0;
        assert!(formal.generate_soul("helper").contains("formal"));
    }

    #[test]
    fn generate_soul_toggles() {
        let mut with_emoji = Persona::default();
        with_emoji.uses_emoji = true;
        let soul = with_emoji.generate_soul("bot");
        assert!(soul.contains("Use emoji"));

        let mut no_emoji = Persona::default();
        no_emoji.uses_emoji = false;
        let soul = no_emoji.generate_soul("bot");
        assert!(soul.contains("Do not use emoji"));
    }

    #[test]
    fn preset_coverage() {
        for name in Persona::preset_names() {
            let persona = Persona::for_role(name);
            assert!(persona.is_some(), "missing preset for role: {name}");
            let persona = persona.unwrap();
            // All dimensions should be in valid range
            assert!((0.0..=1.0).contains(&persona.formality));
            assert!((0.0..=1.0).contains(&persona.verbosity));
            assert!((0.0..=1.0).contains(&persona.warmth));
            assert!((0.0..=1.0).contains(&persona.humor));
            assert!((0.0..=1.0).contains(&persona.confidence));
            assert!((0.0..=1.0).contains(&persona.curiosity));
            // Each preset should have a tone
            assert!(persona.tone.is_some(), "preset {name} missing tone");
        }
    }

    #[test]
    fn unknown_role_returns_none() {
        assert!(Persona::for_role("unicorn").is_none());
    }

    #[test]
    fn toml_roundtrip() {
        let original = Persona::for_role("coder").unwrap();
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: Persona = toml::from_str(&toml_str).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn default_persona_serde() {
        let original = Persona::default();
        let toml_str = toml::to_string_pretty(&original).unwrap();
        let parsed: Persona = toml::from_str(&toml_str).unwrap();
        assert_eq!(original, parsed);
    }
}
