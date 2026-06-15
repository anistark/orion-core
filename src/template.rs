use crate::messages::{Message, Role};
use crate::tools::ToolSchema;

/// Chat prompt template for formatting conversations.
///
/// Implementations convert system prompts, messages, and tool schemas
/// into the prompt format expected by a model family (ChatML, Llama 3, etc.).
/// Also used by the context pipeline for accurate token budget accounting.
pub trait ChatTemplate: Send + Sync {
    /// Template identifier (e.g., "chatml", "llama3", "mistral").
    fn name(&self) -> &str;

    /// Format a complete prompt from system prompt, messages, and tools.
    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String;

    /// Format the system block (system prompt + tool definitions).
    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String;

    /// Wrap a single conversation message in template markers.
    /// Returns empty string for system messages (handled by `format_system`).
    fn format_message(&self, message: &Message) -> String;

    /// The string appended after all messages to open the assistant's turn.
    fn assistant_prefix(&self) -> &str;
}

/// Render the shared tool-instruction block.
///
/// Every template advertises tools the same way (a description list plus a
/// `tool_call` JSON convention) so the agent's tool-call parser stays
/// format-agnostic. Returns an empty string when there are no tools.
fn render_tools(tools: &[ToolSchema]) -> String {
    if tools.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n\nYou have access to the following tools:\n\n");
    for tool in tools {
        s.push_str(&format!(
            "### {}\n{}\nParameters: {}\n\n",
            tool.name,
            tool.description,
            serde_json::to_string_pretty(&tool.parameters).unwrap_or_default()
        ));
    }
    s.push_str(
        "To use a tool, respond with a JSON block:\n\
         ```tool_call\n\
         {\"name\": \"tool_name\", \"arguments\": {...}}\n\
         ```\n",
    );
    s
}

/// Render a tool result as plain text for templates that lack a dedicated
/// tool role (Mistral, Alpaca, Vicuna). The agent surfaces these inside a
/// user turn so the model can read the observation.
fn render_tool_result(message: &Message) -> String {
    format!("[Tool result]\n{}", message.content)
}

// --- ChatML ----------------------------------------------------------------

/// ChatML template format (default).
///
/// ```text
/// <|im_start|>system
/// {system_prompt}<|im_end|>
/// <|im_start|>user
/// {message}<|im_end|>
/// <|im_start|>assistant
/// ```
pub struct ChatMLTemplate;

impl ChatTemplate for ChatMLTemplate {
    fn name(&self) -> &str {
        "chatml"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from("<|im_start|>system\n");
        s.push_str(system_prompt);
        s.push_str(&render_tools(tools));
        s.push_str("<|im_end|>\n");
        s
    }

    fn format_message(&self, message: &Message) -> String {
        let role_str = match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::ToolResult => "tool",
            Role::ToolCall => "assistant",
            Role::System => return String::new(),
        };
        format!("<|im_start|>{role_str}\n{}<|im_end|>\n", message.content)
    }

    fn assistant_prefix(&self) -> &str {
        "<|im_start|>assistant\n"
    }
}

// --- Llama 3 ---------------------------------------------------------------

/// Llama 3 / 3.1 template format.
///
/// ```text
/// <|begin_of_text|><|start_header_id|>system<|end_header_id|>
///
/// {system_prompt}<|eot_id|><|start_header_id|>user<|end_header_id|>
///
/// {message}<|eot_id|><|start_header_id|>assistant<|end_header_id|>
///
/// ```
pub struct Llama3Template;

impl Llama3Template {
    fn header(&self, role: &str, content: &str) -> String {
        format!("<|start_header_id|>{role}<|end_header_id|>\n\n{content}<|eot_id|>")
    }
}

impl ChatTemplate for Llama3Template {
    fn name(&self) -> &str {
        "llama3"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut content = String::from(system_prompt);
        content.push_str(&render_tools(tools));
        format!("<|begin_of_text|>{}", self.header("system", &content))
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => self.header("user", &message.content),
            Role::Assistant | Role::ToolCall => self.header("assistant", &message.content),
            // Llama 3.1 uses the `ipython` role for tool outputs.
            Role::ToolResult => self.header("ipython", &message.content),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "<|start_header_id|>assistant<|end_header_id|>\n\n"
    }
}

// --- Mistral / Mixtral -----------------------------------------------------

/// Mistral / Mixtral instruct template.
///
/// ```text
/// <s>[INST] {system}
///
/// {user} [/INST] {assistant}</s>[INST] {user_2} [/INST]
/// ```
///
/// Mistral has no dedicated system role — the system prompt is merged into
/// the first user instruction. Because that merge needs cross-message state,
/// `format()` is implemented directly; `format_system` / `format_message`
/// return token-representative fragments for context-budget accounting.
pub struct MistralTemplate;

impl MistralTemplate {
    fn system_text(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from(system_prompt);
        s.push_str(&render_tools(tools));
        s
    }
}

impl ChatTemplate for MistralTemplate {
    fn name(&self) -> &str {
        "mistral"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        let mut out = String::from("<s>");
        let mut system_pending = !system.is_empty();

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            match msg.role {
                Role::User | Role::ToolResult => {
                    let body = if msg.role == Role::ToolResult {
                        render_tool_result(msg)
                    } else {
                        msg.content.clone()
                    };
                    let body = if system_pending {
                        system_pending = false;
                        format!("{system}\n\n{body}")
                    } else {
                        body
                    };
                    out.push_str(&format!("[INST] {body} [/INST]"));
                }
                Role::Assistant | Role::ToolCall => {
                    out.push_str(&format!(" {}</s>", msg.content));
                }
                Role::System => {}
            }
        }

        out
    }

    // NOTE: `format_system` / `format_message` below are token-representative,
    // not byte-exact. The context pipeline sums them to estimate cost before
    // pruning; the prompt actually sent to the model always comes from
    // `format()` above. Because Mistral fuses the system prompt into the first
    // user `[INST]` (a whole-conversation operation these per-fragment hooks
    // can't see), concatenating the fragments emits one extra `[INST]` when a
    // system prompt is present — so the budget over-counts by ~2-3 tokens.
    // That's deliberately conservative (reserves a hair more headroom, never
    // under-counts). ChatML/Llama3/Alpaca/Vicuna have no such merge, so their
    // fragments are exact.
    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        if system.is_empty() {
            String::from("<s>")
        } else {
            format!("<s>[INST] {system}\n\n")
        }
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("[INST] {} [/INST]", message.content),
            Role::Assistant | Role::ToolCall => format!(" {}</s>", message.content),
            Role::ToolResult => format!("[INST] {} [/INST]", render_tool_result(message)),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        " "
    }
}

// --- Alpaca ----------------------------------------------------------------

/// Alpaca instruction template.
///
/// ```text
/// {system}
///
/// ### Instruction:
/// {user}
///
/// ### Response:
/// {assistant}
/// ```
pub struct AlpacaTemplate;

const ALPACA_PREAMBLE: &str =
    "Below is an instruction that describes a task. Write a response that appropriately completes the request.";

impl ChatTemplate for AlpacaTemplate {
    fn name(&self) -> &str {
        "alpaca"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let preamble = if system_prompt.is_empty() {
            ALPACA_PREAMBLE
        } else {
            system_prompt
        };
        format!("{preamble}{}\n\n", render_tools(tools))
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("### Instruction:\n{}\n\n", message.content),
            Role::Assistant | Role::ToolCall => format!("### Response:\n{}\n\n", message.content),
            Role::ToolResult => {
                format!("### Instruction:\n{}\n\n", render_tool_result(message))
            }
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "### Response:\n"
    }
}

// --- Vicuna ----------------------------------------------------------------

/// Vicuna v1.1 template.
///
/// ```text
/// {system} USER: {user} ASSISTANT: {assistant}</s>USER: {user_2} ASSISTANT:
/// ```
pub struct VicunaTemplate;

const VICUNA_PREAMBLE: &str = "A chat between a curious user and an artificial intelligence assistant. The assistant gives helpful, detailed, and polite answers to the user's questions.";

impl ChatTemplate for VicunaTemplate {
    fn name(&self) -> &str {
        "vicuna"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let preamble = if system_prompt.is_empty() {
            VICUNA_PREAMBLE
        } else {
            system_prompt
        };
        format!("{preamble}{} ", render_tools(tools))
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("USER: {} ", message.content),
            Role::Assistant | Role::ToolCall => format!("ASSISTANT: {}</s>", message.content),
            Role::ToolResult => format!("USER: {} ", render_tool_result(message)),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "ASSISTANT: "
    }
}

// --- Gemma -----------------------------------------------------------------

/// Gemma / Gemma 2 instruction template.
///
/// ```text
/// <bos><start_of_turn>user
/// {system}
///
/// {user}<end_of_turn>
/// <start_of_turn>model
/// {assistant}<end_of_turn>
/// <start_of_turn>model
/// ```
///
/// Gemma has no dedicated system role, so (like Mistral) the system prompt is
/// merged into the first user turn; `format()` is implemented directly while
/// `format_system` / `format_message` return token-representative fragments.
pub struct GemmaTemplate;

impl GemmaTemplate {
    fn system_text(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from(system_prompt);
        s.push_str(&render_tools(tools));
        s
    }
}

impl ChatTemplate for GemmaTemplate {
    fn name(&self) -> &str {
        "gemma"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        let mut out = String::from("<bos>");
        let mut system_pending = !system.is_empty();

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            match msg.role {
                Role::User | Role::ToolResult => {
                    let body = if msg.role == Role::ToolResult {
                        render_tool_result(msg)
                    } else {
                        msg.content.clone()
                    };
                    let body = if system_pending {
                        system_pending = false;
                        format!("{system}\n\n{body}")
                    } else {
                        body
                    };
                    out.push_str(&format!("<start_of_turn>user\n{body}<end_of_turn>\n"));
                }
                Role::Assistant | Role::ToolCall => {
                    out.push_str(&format!(
                        "<start_of_turn>model\n{}<end_of_turn>\n",
                        msg.content
                    ));
                }
                Role::System => {}
            }
        }

        out.push_str(self.assistant_prefix());
        out
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        if system.is_empty() {
            String::from("<bos>")
        } else {
            format!("<bos><start_of_turn>user\n{system}\n\n")
        }
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("<start_of_turn>user\n{}<end_of_turn>\n", message.content),
            Role::Assistant | Role::ToolCall => {
                format!("<start_of_turn>model\n{}<end_of_turn>\n", message.content)
            }
            Role::ToolResult => format!(
                "<start_of_turn>user\n{}<end_of_turn>\n",
                render_tool_result(message)
            ),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "<start_of_turn>model\n"
    }
}

// --- Phi-3 -----------------------------------------------------------------

/// Phi-3 template.
///
/// ```text
/// <|system|>
/// {system}<|end|>
/// <|user|>
/// {user}<|end|>
/// <|assistant|>
/// {assistant}<|end|>
/// <|assistant|>
/// ```
///
/// The system block is omitted entirely when there is no system prompt or tool
/// schema to advertise.
pub struct Phi3Template;

impl ChatTemplate for Phi3Template {
    fn name(&self) -> &str {
        "phi3"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let tools_block = render_tools(tools);
        if system_prompt.is_empty() && tools_block.is_empty() {
            return String::new();
        }
        format!("<|system|>\n{system_prompt}{tools_block}<|end|>\n")
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("<|user|>\n{}<|end|>\n", message.content),
            Role::Assistant | Role::ToolCall => {
                format!("<|assistant|>\n{}<|end|>\n", message.content)
            }
            Role::ToolResult => format!("<|user|>\n{}<|end|>\n", render_tool_result(message)),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "<|assistant|>\n"
    }
}

// --- DeepSeek --------------------------------------------------------------

/// DeepSeek BOS token (the `▁` is U+2581, the SentencePiece space marker).
const DEEPSEEK_BOS: &str = "<｜begin▁of▁sentence｜>";
/// DeepSeek EOS token.
const DEEPSEEK_EOS: &str = "<｜end▁of▁sentence｜>";

/// DeepSeek-LLM chat template.
///
/// ```text
/// <｜begin▁of▁sentence｜>{system}
///
/// User: {user}
///
/// Assistant: {assistant}<｜end▁of▁sentence｜>
/// ```
///
/// The system prompt is emitted bare (no role marker) after the BOS token, then
/// turns alternate `User:` / `Assistant:`. (DeepSeek-Coder uses an Alpaca-style
/// `### Instruction:` format instead and resolves to [`AlpacaTemplate`].)
pub struct DeepSeekTemplate;

impl ChatTemplate for DeepSeekTemplate {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from(system_prompt);
        s.push_str(&render_tools(tools));
        if s.is_empty() {
            String::from(DEEPSEEK_BOS)
        } else {
            format!("{DEEPSEEK_BOS}{s}\n\n")
        }
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("User: {}\n\n", message.content),
            Role::Assistant | Role::ToolCall => {
                format!("Assistant: {}{DEEPSEEK_EOS}", message.content)
            }
            Role::ToolResult => format!("User: {}\n\n", render_tool_result(message)),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "Assistant:"
    }
}

// --- Command-R -------------------------------------------------------------

/// Cohere Command-R / Command-R+ template.
///
/// ```text
/// <BOS_TOKEN><|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>{system}<|END_OF_TURN_TOKEN|>\
/// <|START_OF_TURN_TOKEN|><|USER_TOKEN|>{user}<|END_OF_TURN_TOKEN|>\
/// <|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>{assistant}<|END_OF_TURN_TOKEN|>\
/// <|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>
/// ```
pub struct CommandRTemplate;

impl ChatTemplate for CommandRTemplate {
    fn name(&self) -> &str {
        "command-r"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let mut prompt = self.format_system(system_prompt, tools);

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            prompt.push_str(&self.format_message(msg));
        }

        prompt.push_str(self.assistant_prefix());
        prompt
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from(system_prompt);
        s.push_str(&render_tools(tools));
        if s.is_empty() {
            String::from("<BOS_TOKEN>")
        } else {
            format!("<BOS_TOKEN><|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>{s}<|END_OF_TURN_TOKEN|>")
        }
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!(
                "<|START_OF_TURN_TOKEN|><|USER_TOKEN|>{}<|END_OF_TURN_TOKEN|>",
                message.content
            ),
            Role::Assistant | Role::ToolCall => format!(
                "<|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>{}<|END_OF_TURN_TOKEN|>",
                message.content
            ),
            Role::ToolResult => format!(
                "<|START_OF_TURN_TOKEN|><|USER_TOKEN|>{}<|END_OF_TURN_TOKEN|>",
                render_tool_result(message)
            ),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        "<|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>"
    }
}

// --- Llama 2 ---------------------------------------------------------------

/// Llama 2 chat template.
///
/// ```text
/// <s>[INST] <<SYS>>
/// {system}
/// <</SYS>>
///
/// {user} [/INST] {assistant} </s><s>[INST] {user_2} [/INST]
/// ```
///
/// Like Mistral, Llama 2 has no system role — the system prompt lives in a
/// `<<SYS>>` block inside the first instruction — so `format()` is implemented
/// directly while the per-message hooks are token-representative.
pub struct Llama2Template;

impl Llama2Template {
    fn system_text(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let mut s = String::from(system_prompt);
        s.push_str(&render_tools(tools));
        s
    }
}

impl ChatTemplate for Llama2Template {
    fn name(&self) -> &str {
        "llama2"
    }

    fn format(&self, system_prompt: &str, messages: &[Message], tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        let mut out = String::new();
        let mut system_pending = !system.is_empty();

        for msg in messages.iter().filter(|m| m.role != Role::System) {
            match msg.role {
                Role::User | Role::ToolResult => {
                    let body = if msg.role == Role::ToolResult {
                        render_tool_result(msg)
                    } else {
                        msg.content.clone()
                    };
                    let inst = if system_pending {
                        system_pending = false;
                        format!("<<SYS>>\n{system}\n<</SYS>>\n\n{body}")
                    } else {
                        body
                    };
                    out.push_str(&format!("<s>[INST] {inst} [/INST]"));
                }
                Role::Assistant | Role::ToolCall => {
                    out.push_str(&format!(" {} </s>", msg.content));
                }
                Role::System => {}
            }
        }

        out
    }

    fn format_system(&self, system_prompt: &str, tools: &[ToolSchema]) -> String {
        let system = self.system_text(system_prompt, tools);
        if system.is_empty() {
            String::from("<s>")
        } else {
            format!("<s>[INST] <<SYS>>\n{system}\n<</SYS>>\n\n")
        }
    }

    fn format_message(&self, message: &Message) -> String {
        match message.role {
            Role::User => format!("<s>[INST] {} [/INST]", message.content),
            Role::Assistant | Role::ToolCall => format!(" {} </s>", message.content),
            Role::ToolResult => format!("<s>[INST] {} [/INST]", render_tool_result(message)),
            Role::System => String::new(),
        }
    }

    fn assistant_prefix(&self) -> &str {
        " "
    }
}

// --- Selection -------------------------------------------------------------

/// Resolve a template by name (used by the manual override dropdown).
///
/// Returns `None` for names without an implementation so the caller can fall
/// back to GGUF auto-detection or ChatML.
pub fn template_from_name(name: &str) -> Option<Box<dyn ChatTemplate>> {
    match name.trim().to_ascii_lowercase().as_str() {
        "chatml" => Some(Box::new(ChatMLTemplate)),
        "llama3" | "llama-3" | "llama3.1" | "llama-3.1" => Some(Box::new(Llama3Template)),
        "llama2" | "llama-2" | "llama 2" => Some(Box::new(Llama2Template)),
        "mistral" | "mixtral" => Some(Box::new(MistralTemplate)),
        "alpaca" => Some(Box::new(AlpacaTemplate)),
        "vicuna" => Some(Box::new(VicunaTemplate)),
        "gemma" | "gemma2" => Some(Box::new(GemmaTemplate)),
        "phi3" | "phi-3" | "phi" => Some(Box::new(Phi3Template)),
        "deepseek" | "deepseek-llm" => Some(Box::new(DeepSeekTemplate)),
        "command-r" | "commandr" | "command_r" | "cohere" => Some(Box::new(CommandRTemplate)),
        // Returning `None` lets the caller fall back to GGUF auto-detection
        // rather than forcing the wrong format. Add a case here to promote a
        // family from "auto" to an explicit override.
        _ => None,
    }
}

/// Select a chat template based on GGUF metadata template string.
///
/// Inspects the Jinja template string and returns the matching implementation.
/// Falls back to ChatML when no match is found or no template is provided.
pub fn detect_template(gguf_template: Option<&str>) -> Box<dyn ChatTemplate> {
    if let Some(tmpl) = gguf_template {
        // Order matters: check the most specific markers first. Llama 3's header
        // tokens are checked before ChatML (some Llama 3 GGUFs embed both);
        // Llama 2's `<<SYS>>` is checked before Mistral's shared `[INST]`.
        if tmpl.contains("<|start_header_id|>") || tmpl.contains("<|begin_of_text|>") {
            return Box::new(Llama3Template);
        }
        if tmpl.contains("<|START_OF_TURN_TOKEN|>") || tmpl.contains("<|CHATBOT_TOKEN|>") {
            return Box::new(CommandRTemplate);
        }
        if tmpl.contains("<|im_start|>") {
            return Box::new(ChatMLTemplate);
        }
        if tmpl.contains("<|assistant|>")
            && (tmpl.contains("<|user|>") || tmpl.contains("<|system|>"))
        {
            return Box::new(Phi3Template);
        }
        if tmpl.contains("<start_of_turn>") {
            return Box::new(GemmaTemplate);
        }
        // Llama 2's `<<SYS>>` block must be matched before Mistral's `[INST]`.
        if tmpl.contains("<<SYS>>") {
            return Box::new(Llama2Template);
        }
        if tmpl.contains("[INST]") {
            return Box::new(MistralTemplate);
        }
        if tmpl.contains("### Instruction:") {
            return Box::new(AlpacaTemplate);
        }
        // DeepSeek-LLM chat uses title-case `User:` / `Assistant:` (Vicuna is
        // upper-case), optionally with its `▁`-marked sentence tokens.
        if (tmpl.contains("User:") && tmpl.contains("Assistant:")) || tmpl.contains("▁of▁sentence")
        {
            return Box::new(DeepSeekTemplate);
        }
        if tmpl.contains("ASSISTANT:") && tmpl.contains("USER:") {
            return Box::new(VicunaTemplate);
        }
        log::debug!(
            "Unknown chat template format (len={}), falling back to ChatML",
            tmpl.len()
        );
    }
    Box::new(ChatMLTemplate)
}
