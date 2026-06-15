use orion_core::messages::Message;
use orion_core::template::{
    detect_template, template_from_name, AlpacaTemplate, ChatMLTemplate, ChatTemplate,
    CommandRTemplate, DeepSeekTemplate, GemmaTemplate, Llama2Template, Llama3Template,
    MistralTemplate, Phi3Template, VicunaTemplate,
};
use orion_core::tools::ToolSchema;

fn convo() -> Vec<Message> {
    vec![
        Message::user("1", "hi"),
        Message::assistant("2", "hello"),
        Message::user("3", "how are you"),
    ]
}

fn make_tool() -> ToolSchema {
    ToolSchema {
        name: "search".into(),
        description: "Search the web".into(),
        parameters: serde_json::json!({"type": "object"}),
    }
}

// --- ChatML ----------------------------------------------------------------

#[test]
fn chatml_format() {
    let t = ChatMLTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<|im_start|>system\nsys<|im_end|>\n\
         <|im_start|>user\nhi<|im_end|>\n\
         <|im_start|>assistant\nhello<|im_end|>\n\
         <|im_start|>user\nhow are you<|im_end|>\n\
         <|im_start|>assistant\n"
    );
}

// --- Llama 3 ---------------------------------------------------------------

#[test]
fn llama3_format() {
    let t = Llama3Template;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<|begin_of_text|>\
         <|start_header_id|>system<|end_header_id|>\n\nsys<|eot_id|>\
         <|start_header_id|>user<|end_header_id|>\n\nhi<|eot_id|>\
         <|start_header_id|>assistant<|end_header_id|>\n\nhello<|eot_id|>\
         <|start_header_id|>user<|end_header_id|>\n\nhow are you<|eot_id|>\
         <|start_header_id|>assistant<|end_header_id|>\n\n"
    );
}

#[test]
fn llama3_begins_with_bos_once() {
    let t = Llama3Template;
    let out = t.format("sys", &convo(), &[]);
    assert!(out.starts_with("<|begin_of_text|>"));
    assert_eq!(out.matches("<|begin_of_text|>").count(), 1);
    assert!(out.ends_with("<|start_header_id|>assistant<|end_header_id|>\n\n"));
}

// --- Mistral ---------------------------------------------------------------

#[test]
fn mistral_merges_system_into_first_user() {
    let t = MistralTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<s>[INST] sys\n\nhi [/INST] hello</s>[INST] how are you [/INST]"
    );
}

#[test]
fn mistral_without_system_has_no_inst_prefix_text() {
    let t = MistralTemplate;
    let out = t.format("", &convo(), &[]);
    assert_eq!(
        out,
        "<s>[INST] hi [/INST] hello</s>[INST] how are you [/INST]"
    );
    assert_eq!(out.matches("<s>").count(), 1);
}

// --- Alpaca ----------------------------------------------------------------

#[test]
fn alpaca_format() {
    let t = AlpacaTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "sys\n\n\
         ### Instruction:\nhi\n\n\
         ### Response:\nhello\n\n\
         ### Instruction:\nhow are you\n\n\
         ### Response:\n"
    );
}

#[test]
fn alpaca_uses_default_preamble_without_system() {
    let t = AlpacaTemplate;
    let out = t.format("", &convo(), &[]);
    assert!(out.starts_with("Below is an instruction that describes a task."));
}

// --- Vicuna ----------------------------------------------------------------

#[test]
fn vicuna_format() {
    let t = VicunaTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "sys USER: hi ASSISTANT: hello</s>USER: how are you ASSISTANT: "
    );
}

#[test]
fn vicuna_uses_default_preamble_without_system() {
    let t = VicunaTemplate;
    let out = t.format("", &convo(), &[]);
    assert!(out.starts_with("A chat between a curious user"));
}

// --- Gemma -----------------------------------------------------------------

#[test]
fn gemma_merges_system_into_first_user() {
    let t = GemmaTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<bos><start_of_turn>user\nsys\n\nhi<end_of_turn>\n\
         <start_of_turn>model\nhello<end_of_turn>\n\
         <start_of_turn>user\nhow are you<end_of_turn>\n\
         <start_of_turn>model\n"
    );
}

#[test]
fn gemma_without_system_has_single_bos() {
    let t = GemmaTemplate;
    let out = t.format("", &convo(), &[]);
    assert!(out.starts_with("<bos><start_of_turn>user\nhi<end_of_turn>"));
    assert_eq!(out.matches("<bos>").count(), 1);
    assert!(out.ends_with("<start_of_turn>model\n"));
}

// --- Phi-3 -----------------------------------------------------------------

#[test]
fn phi3_format() {
    let t = Phi3Template;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<|system|>\nsys<|end|>\n\
         <|user|>\nhi<|end|>\n\
         <|assistant|>\nhello<|end|>\n\
         <|user|>\nhow are you<|end|>\n\
         <|assistant|>\n"
    );
}

#[test]
fn phi3_omits_system_block_when_empty() {
    let t = Phi3Template;
    let out = t.format("", &convo(), &[]);
    assert!(!out.contains("<|system|>"));
    assert!(out.starts_with("<|user|>\nhi<|end|>\n"));
}

// --- DeepSeek --------------------------------------------------------------

#[test]
fn deepseek_format() {
    let t = DeepSeekTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<｜begin▁of▁sentence｜>sys\n\n\
         User: hi\n\n\
         Assistant: hello<｜end▁of▁sentence｜>\
         User: how are you\n\n\
         Assistant:"
    );
}

// --- Command-R -------------------------------------------------------------

#[test]
fn command_r_format() {
    let t = CommandRTemplate;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<BOS_TOKEN><|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>sys<|END_OF_TURN_TOKEN|>\
         <|START_OF_TURN_TOKEN|><|USER_TOKEN|>hi<|END_OF_TURN_TOKEN|>\
         <|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>hello<|END_OF_TURN_TOKEN|>\
         <|START_OF_TURN_TOKEN|><|USER_TOKEN|>how are you<|END_OF_TURN_TOKEN|>\
         <|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>"
    );
}

// --- Llama 2 ---------------------------------------------------------------

#[test]
fn llama2_merges_system_into_first_inst() {
    let t = Llama2Template;
    let out = t.format("sys", &convo(), &[]);
    assert_eq!(
        out,
        "<s>[INST] <<SYS>>\nsys\n<</SYS>>\n\nhi [/INST] hello </s>\
         <s>[INST] how are you [/INST]"
    );
}

#[test]
fn llama2_without_system_has_no_sys_block() {
    let t = Llama2Template;
    let out = t.format("", &convo(), &[]);
    assert_eq!(
        out,
        "<s>[INST] hi [/INST] hello </s><s>[INST] how are you [/INST]"
    );
    assert!(!out.contains("<<SYS>>"));
}

// --- Tool rendering --------------------------------------------------------

#[test]
fn tools_rendered_in_each_system_block() {
    let tools = vec![make_tool()];
    for t in template_set() {
        let sys = t.format_system("sys", &tools);
        assert!(
            sys.contains("You have access to the following tools:"),
            "{} should render tools in system block",
            t.name()
        );
        assert!(
            sys.contains("```tool_call"),
            "{} should render tool-call convention",
            t.name()
        );
    }
}

// --- Detection -------------------------------------------------------------

#[test]
fn detect_from_gguf_markers() {
    assert_eq!(detect_template(Some("<|im_start|>user")).name(), "chatml");
    assert_eq!(
        detect_template(Some("<|start_header_id|>system<|end_header_id|>")).name(),
        "llama3"
    );
    assert_eq!(
        detect_template(Some("<|begin_of_text|> hello")).name(),
        "llama3"
    );
    assert_eq!(
        detect_template(Some("[INST] {{ x }} [/INST]")).name(),
        "mistral"
    );
    assert_eq!(
        detect_template(Some("### Instruction:\n{{ x }}")).name(),
        "alpaca"
    );
    assert_eq!(
        detect_template(Some("USER: {{ x }} ASSISTANT:")).name(),
        "vicuna"
    );
    assert_eq!(
        detect_template(Some("<start_of_turn>user\n{{ x }}<end_of_turn>")).name(),
        "gemma"
    );
    assert_eq!(
        detect_template(Some(
            "<|system|>\n{{ s }}<|end|>\n<|user|>\n{{ x }}<|assistant|>"
        ))
        .name(),
        "phi3"
    );
    assert_eq!(
        detect_template(Some("<|START_OF_TURN_TOKEN|><|USER_TOKEN|>{{ x }}")).name(),
        "command-r"
    );
    assert_eq!(
        detect_template(Some("{{ bos }}User: {{ x }}\n\nAssistant: {{ y }}")).name(),
        "deepseek"
    );
    // Llama 2's <<SYS>> must win over the shared [INST] (Mistral) marker.
    assert_eq!(
        detect_template(Some(
            "<s>[INST] <<SYS>>\n{{ s }}\n<</SYS>>\n\n{{ x }} [/INST]"
        ))
        .name(),
        "llama2"
    );
}

#[test]
fn detect_falls_back_to_chatml() {
    assert_eq!(detect_template(None).name(), "chatml");
    assert_eq!(detect_template(Some("some unknown jinja")).name(), "chatml");
}

#[test]
fn template_from_name_resolves_known_aliases() {
    assert_eq!(template_from_name("chatml").unwrap().name(), "chatml");
    assert_eq!(template_from_name("llama3").unwrap().name(), "llama3");
    assert_eq!(template_from_name("Llama-3.1").unwrap().name(), "llama3");
    assert_eq!(template_from_name("MISTRAL").unwrap().name(), "mistral");
    assert_eq!(template_from_name("mixtral").unwrap().name(), "mistral");
    assert_eq!(template_from_name("alpaca").unwrap().name(), "alpaca");
    assert_eq!(template_from_name("vicuna").unwrap().name(), "vicuna");
    assert_eq!(template_from_name("llama2").unwrap().name(), "llama2");
    assert_eq!(template_from_name("Llama-2").unwrap().name(), "llama2");
    assert_eq!(template_from_name("gemma").unwrap().name(), "gemma");
    assert_eq!(template_from_name("phi-3").unwrap().name(), "phi3");
    assert_eq!(template_from_name("deepseek").unwrap().name(), "deepseek");
    assert_eq!(template_from_name("Command-R").unwrap().name(), "command-r");
    assert_eq!(template_from_name("cohere").unwrap().name(), "command-r");
}

#[test]
fn template_from_name_returns_none_for_unimplemented() {
    assert!(template_from_name("qwen").is_none());
    assert!(template_from_name("yi").is_none());
    assert!(template_from_name("").is_none());
}

fn template_set() -> Vec<Box<dyn ChatTemplate>> {
    vec![
        Box::new(ChatMLTemplate),
        Box::new(Llama3Template),
        Box::new(Llama2Template),
        Box::new(MistralTemplate),
        Box::new(AlpacaTemplate),
        Box::new(VicunaTemplate),
        Box::new(GemmaTemplate),
        Box::new(Phi3Template),
        Box::new(DeepSeekTemplate),
        Box::new(CommandRTemplate),
    ]
}
