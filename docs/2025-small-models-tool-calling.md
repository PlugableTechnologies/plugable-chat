Here is a model-focused, cross-vendor guide to tool/function/MCP calling as of 30 Nov 2025, with enough detail to actually implement adapters.

I will organize by:

1. Core patterns that almost everyone converged on
2. A canonical internal abstraction you can target
3. Model family specifics

   * Microsoft Phi
   * IBM Granite
   * Alibaba Qwen
   * OpenAI GPT-OSS
   * Llama 3.x and variants
   * Gemma
   * Mistral
   * Ollama and Hermes-style formats
4. Practical comparison and adapter advice

---

## 1. Core patterns

Across vendors, you see only a few real patterns.

### 1.1 Two main IO shapes

1. **Native structured tool calls**

   The model returns a structured object out-of-band from the natural text, usually with something like:

   ```json
   {
     "role": "assistant",
     "tool_calls": [
       {
         "id": "call_1",
         "type": "function",
         "function": {
           "name": "get_weather",
           "arguments": "{\"location\":\"Seattle\"}"
         }
       }
     ]
   }
   ```

   or an equivalent `ToolCall` list on the SDK side. This is the pattern used by:

   * OpenAI chat tools
   * Mistral official APIs
   * Azure Phi 4 tools API
   * Ollama tool support when the model output matches a tool template
   * Some Granite and Granite-NIM integrations

   In most cases, arguments are a JSON string that your runtime must parse.([Mistral AI][1])

2. **Prompt-engineered string formats**

   The model is instructed in text to output something like:

   ```text
   <function=get_weather>{"location": "Tokyo, JP"}</function>
   ```

   or

   ```json
   {"name": "get_weather", "parameters": {"location": "Tokyo"}}
   ```

   Your code then parses this string into a tool call. This is how:

   * Llama 3.x zero shot function calling works by default
   * Gemma function calling works
   * Many Qwen deployments are used, especially with Hermes format
   * Hermes-finetuned models (Hermes, some Gemma and Llama variants) operate

   For these, the "tool format" is mostly baked into a chat template, not the model API itself.([GitHub][2])

### 1.2 Parallel vs single tool calls

* Some models can output multiple tool calls at once (`tool_calls` array).
* Some, like GPT-OSS, effectively support only one tool call per response, so you loop until there are no more calls.([alde.dev][3])

---

## 2. A canonical internal abstraction

If you normalize everything to this, adapters become mechanical.

```ts
type ToolSchema = {
  name: string
  description?: string
  parameters: JSONSchema
  // optional hints to your own runtime
  type?: string               // eg "code_execution_20250825"
  allowed_callers?: string[]  // eg ["code_execution_20250825"]
  defer_loading?: boolean     // discovered via a tool-search step
}

type ToolCall = {
  toolName: string
  arguments: Record<string, unknown>  // already parsed JSON
  rawSource: string                   // raw model text for debugging
  modelId: string
  kind?: 'normal' | 'code_execution' | 'tool_search'
}
```

Then give each model family a `ModelProfile`:

```ts
type ModelProfile = {
  id: string
  match: RegExp
  buildPrompt: (ctx: ChatContext, tools: ToolSchema[], options: PromptOptions) => ModelInput
  parseToolCalls: (output: ModelOutput) => ToolCall[]
}
```

Where `buildPrompt` and `parseToolCalls` are implemented per family to:

* feed tools in the right way
* extract tool calls in the right way

Everything below is essentially how to implement those two functions for each family.

---

## 3. Model family specifics

### 3.1 Microsoft Phi (Phi 4 and Phi-4-mini)

**Where tool calling is defined**

* Phi 4 models (Phi-4-mini, Phi-4-multimodal) are exposed with tool calling through Azure AI and PhiCookBook examples.([GitHub][4])

**Input format**

On Azure, Phi uses the OpenAI-like `tools` parameter:

```json
{
  "messages": [
    { "role": "system", "content": "You are a helpful assistant." },
    { "role": "user", "content": "What is the weather in Seattle?" }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get the current weather",
        "parameters": {
          "type": "object",
          "properties": {
            "location": { "type": "string" }
          },
          "required": ["location"]
        }
      }
    }
  ]
}
```

**Output format**

Phi uses the same `tool_calls` structure as OpenAI:

```json
{
  "role": "assistant",
  "content": null,
  "tool_calls": [
    {
      "id": "call_1",
      "type": "function",
      "function": {
        "name": "get_weather",
        "arguments": "{\"location\": \"Seattle\"}"
      }
    }
  ]
}
```

**Profile behavior**

* `buildPrompt`: just pass `tools` through in the OpenAI style.
* `parseToolCalls`: read `message.tool_calls`, parse `function.arguments` as JSON.

**Gotchas**

* Arguments are always a JSON string. Some examples in the cookbook show the model sometimes emitting trailing commas; best practice is to run a "relaxed JSON" sanitizer before parsing.([GitHub][4])

---

### 3.2 IBM Granite

IBM has both special function-calling variants and general instruct models that can be prompted into tool use.([IBM][5])

**Input format**

On watsonx / Granite tutorials, you define functions in a JSON schema format similar to OpenAI:

```json
{
  "functions": [
    {
      "name": "get_weather",
      "description": "Get the current weather",
      "parameters": {
        "type": "object",
        "properties": {
          "location": { "type": "string" }
        },
        "required": ["location"]
      }
    }
  ],
  "messages": [...]
}
```

In some APIs this is under `tools` rather than `functions`, but the structure is analogous.

**Output patterns**

Granite function-calling variants tend to return one of two shapes:

1. **Raw JSON string in the message content**

   ```json
   {
     "role": "assistant",
     "content": "{\"name\": \"get_weather\", \"arguments\": {\"location\": \"Seattle\"}}"
   }
   ```

2. **Marked JSON according to a template**

   Tutorials often instruct the model to respond with a specific JSON only format. Your code then parses the content string directly.([IBM][5])

Some Granite function calling Hugging Face models (for example `granite-20b-functioncalling`) are trained to output consistent JSON snippets that match the function schema.([Hugging Face][6])

**Profile behavior**

* `buildPrompt`:

  * Put function definitions into the appropriate field (`functions` or `tools`) based on the deployment.
  * Add system text such as "If using a function, answer only with a JSON object matching the function call schema".

* `parseToolCalls`:

  * Try to parse the whole `message.content` as JSON.
  * Expected object shape: `{ "name": string, "arguments": object }`.
  * If you see other text around it, extract the first JSON object and parse that.

**Gotchas**

* Granite examples rely heavily on instruction text rather than a strict API field like `tool_calls`. You need to treat these as prompt-engineered string formats, not native fields, unless you are using a wrapper that converts them.

---

### 3.3 Alibaba Qwen (Qwen2.5, Qwen3, QwQ-32B)

Qwen has explicit docs for function calling and recommends Hermes-style templates in Qwen3.([DeepWiki][7])

**Input format**

There are two main ways people run tools with Qwen:

1. **Framework-driven (Qwen-Agent, vLLM, etc)**

   * You register tools in the framework using a JSON schema.

   * The framework uses an internal chat template that may look like:

     ```text
     <tool>
     {"name": "get_weather", "description": "...", "parameters": {...}}
     ...
     </tool>

     When you want to call a tool, respond with:
     {"name": "<tool_name>", "arguments": { ... }}
     ```

   * Hermes-like tool use is recommended in the Qwen3 docs.([GitHub][8])

2. **Ollama-style templates**

   For some Qwen models in Ollama, the chat template uses tags like `<tool_call> ... </tool_call>`:

   ```text
   <tool_call>
   {"name": "get_weather", "arguments": {"location": "Paris"}}
   </tool_call>
   ```

   QwQ-32B discussions show exactly this pattern.([Hugging Face][9])

**Output format**

* Usually a plain text JSON snippet, often wrapped in a tool tag depending on the template. Examples:

  ```json
  {"name": "get_weather", "arguments": {"location": "Cergy, France"}}
  ```

  or

  ```text
  <tool_call>{"name": "get_weather", "arguments": {"location": "Cergy, France"}}</tool_call>
  ```

* Some Qwen chat templates also support OpenAI style `tool_calls` if you are using an OpenAI compatible server, but this is more about the server than the model itself.

**Profile behavior**

* `buildPrompt`:

  * Include a tool block in the system message or a dedicated `<tool>` section in the template listing JSON schemas.
  * Give explicit instructions: "If calling a function, respond with a single JSON object of the form {"name":..., "arguments": {...}} and nothing else."

* `parseToolCalls`:

  * Strip wrapper tags like `<tool_call>` and `</tool_call>`.
  * Find the first JSON object with keys `name` and `arguments`.
  * Parse `arguments` as an object.

**Gotchas**

* For QwQ-32B type models, some chat templates force exactly one function call and do not support parallel calls.
* Qwen3 docs explicitly mention that function calling is prompt engineered and not a dedicated API field. You must respect the template for reliable behavior.([DeepWiki][10])

---

### 3.4 OpenAI GPT-OSS (8B, 20B)

GPT-OSS is OpenAI's open source series. Tool calling for GPT-OSS is not identical to proprietary GPT-4.x, and depends heavily on the runtime (vLLM, llama.cpp, etc).([Hugging Face][11])

**Input format**

Common patterns:

* When used via vLLM or a similar "OpenAI compatible" server with tool calling support, you define tools with `tools` and `tool_choice` just like proprietary GPT models.

* For local inference (llama.cpp, etc) you often use a Hermes style prompt:

  ```text
  You have access to the following functions:
  {JSON schema list}

  If you call a function, respond only with:
  {"tool_name": "<name>", "tool_args": {...}}
  ```

**Output format**

* GPT-OSS has an internal "tool_call" representation when used with certain parsers, but:

  * Only one tool call per response is supported reliably.
  * No native parallel tool_calls array is emitted. You run in a loop until no more calls are produced.([alde.dev][3])

* Typical text content:

  ```json
  {"tool_name": "get_weather", "tool_args": {"location": "Paris"}}
  ```

Some tools like Ollama or vLLM will parse this and surface a `tool_calls` array in their own format.

**Profile behavior**

* `buildPrompt`:

  * If using a server that already exposes `tools`, just use OpenAI style.
  * Otherwise, add a clear JSON-only instruction like the above.

* `parseToolCalls`:

  * If server exposes `tool_calls`, use that.
  * Else, parse message content as JSON and look for `tool_name` and `tool_args`.
  * Run multiple turns if you want multiple tools, since only one is returned at a time.

**Gotchas**

* GPT-OSS is more sensitive to prompt clarity than proprietary GPT. Many reports show the model narrating something like "Need to use get_weather" instead of emitting a proper tool_call if the template is not strict.([Hugging Face][11])

---

### 3.5 Llama 3.x (Meta) and similar

Meta provides an official prompt format for Llama 3.3 tool calling.([GitHub][2])

**Input format**

Llama zero shot function calling usually puts function definitions into the system message in plain text:

```text
<|begin_of_text|><|start_header_id|>system<|end_header_id|>
You have access to the following functions:
{JSON schema list}

When you choose to call a function, respond ONLY with a JSON object:
{"name": "<function_name>", "parameters": { ... }}
<|eot_id|>
```

User messages follow with the standard chat template.

Ollama's Llama 3.3 template includes a similar line:

> Given the following functions, please respond with a JSON for a function call with its proper arguments … Respond in the format {"name": function name, "parameters": dictionary}.([Ollama][12])

Other recipes like Braintrust's LLaMa 3.1 tools use a tag-based format:

```text
If you choose to call a function ONLY reply in the following format with no prefix or suffix:
<function=get_current_weather>{"location": "Tokyo, JP"}</function>
```

([Braintrust][13])

**Output format**

* Often a bare JSON object:

  ```json
  {"name": "get_weather", "parameters": {"location": "Seattle"}}
  ```

* Or, if following the Braintrust recipe, a tagged call:

  ```text
  <function=get_current_weather>{"location": "Tokyo, JP"}</function>
  ```

**Profile behavior**

* `buildPrompt`:

  * Inject tools into the system message with the Llama 3.x chat template.
  * For your deployment, pick one format and be strict about it:

    * pure JSON `{"name":..., "parameters": {...}}`
    * or tagged `<function=...>{...}</function>`

* `parseToolCalls`:

  * Strip tags if present.
  * Parse JSON and map `name` to `toolName` and `parameters` to `arguments`.

**Gotchas**

* Llama 3.x itself does not know about an API field called `tools`; it just follows whatever your template says. Tools are a template convention, not an ABI.

---

### 3.6 Gemma

Gemma function calling is explicitly prompt based. Google’s docs show you:

* Put function definitions and expected syntax into the prompt.
* Model outputs a string representing the call.([Google AI for Developers][14])

**Input format**

Docs show a pattern like:

```text
You have access to functions:

1. get_product_details
   - parameters:
     - product_id (string)

When you want to call a function, output exactly:

<function_call>{"name": "<function_name>", "arguments": { ... }}</function_call>
```

You then append the user question.

**Output format**

Gemma returns that string, for example:

```text
<function_call>{"name": "get_product_details", "arguments": {"product_id": "1234"}}</function_call>
```

**Profile behavior**

* `buildPrompt`:

  * Render a function definition block plus instructions into the system or first user message, following the examples in Gemma docs or Gemma function calling finetune notebooks.([Google AI for Developers][14])

* `parseToolCalls`:

  * Extract the JSON between `<function_call>` tags.
  * Parse and interpret as `{ name, arguments }`.

**Gotchas**

* There is no general purpose `tools` API field; results depend heavily on your exact instructions.
* There are finetuned GGUF variants explicitly optimized for function calling; they often assume a particular chat template.([Hugging Face][15])

---

### 3.7 Mistral

Mistral has first class tool calling in its SDK and API.([Mistral AI][1])

**Input format**

Using the official Mistral client, you send a `tools` array and `tool_choice`:

```json
{
  "model": "mistral-small",
  "messages": [
    { "role": "user", "content": "Weather in Paris?" }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "get_weather",
        "description": "Get weather",
        "parameters": {
          "type": "object",
          "properties": {
            "location": { "type": "string" }
          },
          "required": ["location"]
        }
      }
    }
  ],
  "tool_choice": "auto"
}
```

NVIDIA NIM examples for Mistral show the same `tools` schema.([NVIDIA Docs][16])

**Output format**

Mistral wraps tool calls in a `ToolCall` object attached to the assistant message:

```json
{
  "role": "assistant",
  "content": null,
  "tool_calls": [
    {
      "id": "call_1",
      "function": {
        "name": "get_weather",
        "arguments": "{\"location\": \"Paris\"}"
      }
    }
  ]
}
```

The `mistral-common` docs describe this `ToolCall` structure explicitly.([mistralai.github.io][17])

**Profile behavior**

* `buildPrompt`:

  * Pass your tool list straight through to the Mistral SDK.
  * Configure `tool_choice` per your policy: `"auto"`, `"none"`, or a specific tool.

* `parseToolCalls`:

  * Iterate the `tool_calls` array on the assistant message, parse each `function.arguments` JSON string.

**Gotchas**

* Mistral tends to follow the tools strictly if you set `tool_choice` to a concrete function name. Leave it as `"auto"` if you want the model to decide whether to call tools.

---

### 3.8 Ollama and Hermes-style formats

Ollama itself is not a model family, but its tool system is important because it normalizes many open models behind one mechanism.([Ollama][18])

**Input format**

You define tools in your Ollama client request or in a model’s Modelfile. The prompt template usually includes an instruction like:

```text
Given the following functions, please respond with a JSON for a function call ...
Respond in the format {"name": function name, "parameters": { ... }}.
```

for models like Llama 3.3, as seen in the llama3.3 template.([Ollama][12])

**Output format**

The model emits something like:

```json
{"name": "query_contact_manager", "parameters": {"query": "Add a new contact..."}}
```

Ollama then:

* parses this according to the template
* converts it into a `tool_calls` structure in the API response
* clears `content` when converting, so content is empty when a tool call is present.([GitHub][19])

This means your code sees a uniform tool_call representation even though the underlying model is just emitting JSON text.

**Hermes**

Hermes models and datasets define a very similar JSON format, sometimes called the Hermes tool use or Hermes function calling format. It is basically:

```json
{"tool_name": "...", "tool_args": {...}}
```

or

```json
{"name": "...", "arguments": {...}}
```

with strict instruction that nothing else should appear in the output. These have been used to finetune many Llama and Qwen variants for tool use.([docs.camel-ai.org][20])

---

## 4. Practical comparison and adapter hints

Here is a compact comparison focused on what your adapter needs to know.

### 4.1 How to inject tool schemas

* **Native `tools` or `functions` field**

  * Microsoft Phi (via Azure)
  * Mistral official APIs and NIM-based deployments
  * Some Granite deployments (watsonx, NIM)
  * GPT-OSS when hosted behind an OpenAI compatible server like vLLM

* **Prompt only**

  * Gemma
  * Llama 3.x when run directly with a custom template
  * Qwen when using Qwen-Agent or Ollama templates
  * Hermes-style fine-tunes

For everything in the second group, your `buildPrompt` must:

1. Insert a function definition block (usually JSON schemas in text).
2. Explicitly specify the output format for tool calls.

### 4.2 How to detect tool calls

* **Check `tool_calls` / `ToolCall` objects**

  * Phi via Azure
  * Mistral
  * Ollama API, once it has parsed the model output
  * Some Granite tooling

* **Parse a JSON snippet in `message.content`**

  * Granite function calling finetunes
  * GPT-OSS plain deployments
  * Llama, Gemma, Qwen, Hermes when using JSON-only output patterns

* **Parse tag wrappers**

  * Formats like `<function=...>{...}</function>` or `<tool_call>...</tool_call>` for Llama, Qwen, Gemma, Braintrust style templates.([Braintrust][13])

### 4.3 Parallel tool calls

* Supported:

  * Mistral tools (explicit `tool_calls` array)
  * Azure Phi tools
  * Ollama tool system, if the model output template allows multiple calls in one JSON list (check per template)

* Typically single call per turn:

  * GPT-OSS (one tool call per response, loop for multiple)([alde.dev][3])
  * Many prompt-based templates which say "ONLY call one function at a time"

Your runtime should not assume parallel tool calls unless the API explicitly gives you an array.

### 4.4 Reasoning content vs tool calls

Some open models, including GPT-OSS and Qwen, may emit "reasoning" text describing which function they intend to call instead of actually outputting a properly formatted JSON object if your prompt is not strict enough.([Hugging Face][11])

Mitigations:

* Clearly instruct:

  * "If you decide to call a function, respond with ONLY the JSON object, with no explanation."
* Use a chat template that reserves a field for "reasoning_content" separate from tool JSON if your runtime supports it, then ignore that field for parsing.


[1]: https://docs.mistral.ai/capabilities/function_calling?utm_source=chatgpt.com "Function Calling - Mistral Docs"
[2]: https://github.com/meta-llama/llama-models/blob/main/models/llama3_3/prompt_format.md?utm_source=chatgpt.com "llama-models/models/llama3_3/prompt_format.md at main - GitHub"
[3]: https://alde.dev/blog/proper-tool-calling-with-gpt-oss/?utm_source=chatgpt.com "Proper tool calling with gpt-oss :: Alde's Blog"
[4]: https://github.com/microsoft/PhiCookBook/blob/main/md/02.Application/07.FunctionCalling/Phi4/FunctionCallingBasic/README.md?utm_source=chatgpt.com "Function calling in Phi-4-mini - GitHub"
[5]: https://www.ibm.com/think/tutorials/granite-function-calling?utm_source=chatgpt.com "Function Calling with Granite Tutorial | IBM"
[6]: https://huggingface.co/ibm-granite/granite-20b-functioncalling?utm_source=chatgpt.com "ibm-granite/granite-20b-functioncalling · Hugging Face"
[7]: https://deepwiki.com/QwenLM/Qwen2.5/2.2-function-calling-and-tool-use?utm_source=chatgpt.com "Function Calling and Tool Use | QwenLM/Qwen2.5 | DeepWiki"
[8]: https://github.com/QwenLM/Qwen3/blob/main/docs/source/framework/function_call.md?utm_source=chatgpt.com "Qwen3/docs/source/framework/function_call.md at main - GitHub"
[9]: https://huggingface.co/Qwen/QwQ-32B/discussions/12?utm_source=chatgpt.com "Qwen/QwQ-32B · Tool-Calling Format - Hugging Face"
[10]: https://deepwiki.com/QwenLM/Qwen3/4.3-function-calling-and-tool-use?utm_source=chatgpt.com "Function Calling and Tool Use | QwenLM/Qwen3 | DeepWiki"
[11]: https://huggingface.co/openai/gpt-oss-20b/discussions/80?utm_source=chatgpt.com "openai/gpt-oss-20b · tool calling not working as expected?"
[12]: https://ollama.com/library/llama3.3/blobs/948af2743fc7?utm_source=chatgpt.com "llama3.3/template"
[13]: https://www.braintrust.dev/docs/cookbook/recipes/LLaMa-3_1-Tools?utm_source=chatgpt.com "Tool calls in LLaMa 3.1 - braintrust.dev"
[14]: https://ai.google.dev/gemma/docs/capabilities/function-calling?utm_source=chatgpt.com "Function calling with Gemma | Google AI for Developers"
[15]: https://huggingface.co/DiTy/gemma-2-9b-it-function-calling-GGUF?utm_source=chatgpt.com "DiTy/gemma-2-9b-it-function-calling-GGUF · Hugging Face"
[16]: https://docs.nvidia.com/nim/vision-language-models/1.3.1/examples/mistral-small-3-2/function-calling.html?utm_source=chatgpt.com "Call Functions (Tools) Using Mistral Small 3.2 24B Instruct 2506"
[17]: https://mistralai.github.io/mistral-common/usage/tools/?utm_source=chatgpt.com "Tools - Mistral-common"
[18]: https://ollama.com/blog/tool-support?utm_source=chatgpt.com "Tool support · Ollama Blog"
[19]: https://github.com/ollama/ollama/issues/8337?utm_source=chatgpt.com "Cannot get a tool call and a message in the same response"
[20]: https://docs.camel-ai.org/cookbooks/data_generation/data_gen_with_real_function_calls_and_hermes_format?utm_source=chatgpt.com "Real Function Calls and Hermes Format Data Generation"
