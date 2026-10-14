---
title: Long-running work
description: Pending tool results, the inbox, resuming a conversation, and child agents.
---

`Agent::prompt` borrows the agent until the run ends, and a tool's `execute` holds
the run until it returns. That is right for a tool that reads a file. It is wrong
for a build that takes ten minutes, a job the user wants to keep chatting through,
or a helper agent doing a task of its own. Four pieces let work outlast a single
run.

## Pending results

A tool that starts something slow can answer straight away with a placeholder:

```rust
async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput> {
    let job = self.jobs.start(args, ctx.call_id()); // the host's own runtime
    Ok(ToolOutput::pending(format!("Started job {job}. Its result will follow.")))
}
```

The placeholder is recorded as the call's result, marked `pending`, so the
conversation stays well formed and the model knows the work is under way. The
call is listed by `agent.pending_calls()` until the agent reads its real result
from the inbox. That is not the moment you call `resolve`: a result waits in the
inbox until the next read, and resolving the same call twice gets the second
result dropped with a `Warning`.

By default the model reads the placeholder and carries on, usually telling the
user the job has started. Set `AgentConfig::pause_on_pending` to end the run
instead: a turn that leaves calls pending emits `Paused` with the waiting calls,
then `AgentEnd`, and the model is not asked to answer around them.

## The inbox

`agent.inbox()` is a cheap, cloneable handle for reaching the agent from outside
its run. Move it to whatever finishes the job:

```rust
let inbox = agent.inbox();

inbox.push("Also check the README.");               // a user message
inbox.resolve(&call_id, "Report written to out/");  // a pending call's result
inbox.reject(&call_id, "The build failed: exit 2"); // or its failure
```

The agent reads the inbox:

- at the start of `prompt`, ahead of the prompt's own text, and at the start of
  `resume`;
- before every model call after the first, so a message pushed mid-run is seen
  on the next step;
- when the model gives its final answer, so anything that landed while it was
  writing is answered in the same run rather than left waiting.

A message becomes a user turn. A result becomes a second tool result for the same
call, headed so the model knows which earlier call it answers, and fires
`ToolResolved`. A result for a call that is not pending is dropped with a
`Warning`.

## Resuming

`agent.resume(backend, tx)` runs the same loop as `prompt` with no new user text,
from where the conversation stands:

```rust
let inbox = agent.inbox();
agent.prompt("Run the nightly report", backend.clone(), tx).await?;

// Later, when the job the tool started finishes:
for call in agent.pending_calls() {
    inbox.resolve(&call.id, "Report written to out/nightly.csv");
}
agent.resume(backend, tx2).await?;
```

Use it when a result lands between runs, to continue after a pause, and to pick
up a conversation after a restart. `resume` answers `Err`, emitting nothing, when
the inbox is empty and `agent.needs_reply()` is false.

**After a restart.** The conversation is the whole of the state. Pending calls are
worked out from the messages, so a conversation saved with its placeholders and
restored with `replace_messages` lists the same `pending_calls()`: the results the
host still owes. If the process died partway through a tool round, the calls that
never got a result are marked failed when the next run starts, with a `Warning`.
They are never run again, since they may already have taken effect.

## Event sequences

A pending call, with the model answering around it (the default):

```text
… ToolExecStart  { tool_call_id, tool_name, args }
  ToolExecEnd    { result: { pending: true, content: "Started job 7…" } }
  MessageStart / MessageEnd   { the placeholder tool result }
  TurnEnd
  TurnStart … MessageEnd { "I've started it." } … TurnEnd
  AgentEnd
```

The same with `pause_on_pending` set:

```text
… ToolExecEnd    { result: { pending: true, … } }
  MessageStart / MessageEnd   { the placeholder tool result }
  TurnEnd
  Paused         { pending: [the call] }
  AgentEnd
```

`inbox.resolve(..)` followed by `resume(..)`:

```text
AgentStart
ToolResolved     { tool_call_id, tool_name, result }
MessageStart / MessageEnd     { the late tool result }
TurnStart
ContextBudget, MessageDelta …, GenerationStats
MessageEnd       { the model's reply }
TurnEnd
AgentEnd         { messages: [late result, reply] }
```

A child run inside a tool call: every event of the child's run, `AgentStart`
to `AgentEnd`, arrives wrapped between the call's `ToolExecStart` and
`ToolExecEnd`:

```text
ToolExecStart    { tool_call_id: "msg-2-call-1", tool_name: "delegate", … }
Nested           { tool_call_id: "msg-2-call-1", event: AgentStart }
Nested           { tool_call_id: "msg-2-call-1", event: MessageDelta { … } }
…
Nested           { tool_call_id: "msg-2-call-1", event: AgentEnd { … } }
ToolExecEnd      { result: { content: the child's reply } }
```

## Child agents

A tool can run another agent and answer with its reply:

```rust
async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> CoreResult<ToolOutput> {
    let mut researcher = Agent::new(AgentConfig {
        system_prompt: "You research one question and report back briefly.".into(),
        ..AgentConfig::default()
    });
    let task = args["task"].as_str().unwrap_or_default();
    let reply = ctx.run_child(&mut researcher, task, self.backend.clone()).await?;
    Ok(ToolOutput::new(reply.content))
}
```

`run_child` streams the child's events live into the parent's as `Nested`, carrying
the id of the call that started it, so a host can draw the child's work inside the
parent's turn. A grandchild's events arrive nested twice.

For the length of the run the child answers to the parent's stop flag: stopping the
parent stops the child, and `run_child` answers `Err(CoreError::Aborted)`. The child
keeps its conversation, so a tool that holds on to it can send it a follow-up later.

A child run is awaited inside the parent's tool call. To let the parent carry on
while a child works, start the child on the host's runtime and answer with
`ToolOutput::pending`, then deliver the child's reply through the parent's inbox.
