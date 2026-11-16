import type { ProviderKind } from "../../lib/types";

// [0.0.7 P5] The ready-made choices of Settings → Endpoints: the cloud AI companies, which
// need an API key, and the servers people run themselves. Picking one fills in its name and
// address. The addresses and key pages are the companies' own (see CHANGELOG.md, 0.0.7 P5).

/** A cloud AI company: chats with its models go to its servers, and it needs an API key. */
export interface Company {
  /** Who the chats go to ("Chats that use its models go to …"). */
  name: string;
  /** The page where an account makes API keys (opened in the browser). */
  keyUrl: string;
  /** That page's address as the form shows it. */
  keyPage: string;
  /** The context size the app plans with when the server does not say (the Rust
   *  `agent::default_context`). */
  defaultContext: number;
}

/** One choice in the form's "Company or server" list. */
export interface EndpointChoice {
  kind: ProviderKind;
  /** What the list says. */
  label: string;
  /** The name a new endpoint gets (the model list says "model · name"). */
  name: string;
  /** The address filled in. */
  baseUrl: string;
  company?: Company;
}

export const COMPANY_CHOICES: EndpointChoice[] = [
  {
    kind: "anthropic",
    label: "Anthropic (Claude)",
    name: "Anthropic",
    baseUrl: "https://api.anthropic.com",
    company: {
      name: "Anthropic",
      keyUrl: "https://platform.claude.com/settings/keys",
      keyPage: "platform.claude.com",
      defaultContext: 200_000,
    },
  },
  {
    kind: "openai",
    label: "OpenAI",
    name: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    company: {
      name: "OpenAI",
      keyUrl: "https://platform.openai.com/api-keys",
      keyPage: "platform.openai.com",
      defaultContext: 128_000,
    },
  },
  {
    kind: "openrouter",
    label: "OpenRouter",
    name: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    company: {
      name: "OpenRouter",
      keyUrl: "https://openrouter.ai/keys",
      keyPage: "openrouter.ai/keys",
      defaultContext: 32_768,
    },
  },
  {
    kind: "abliteration",
    label: "abliteration.ai",
    name: "abliteration.ai",
    baseUrl: "https://api.abliteration.ai/v1",
    company: {
      name: "abliteration.ai",
      keyUrl: "https://abliteration.ai",
      keyPage: "abliteration.ai",
      defaultContext: 32_768,
    },
  },
];

export const SERVER_CHOICES: EndpointChoice[] = [
  { kind: "ollama", label: "Ollama", name: "Ollama", baseUrl: "http://127.0.0.1:11434" },
  { kind: "lmstudio", label: "LM Studio", name: "LM Studio", baseUrl: "http://127.0.0.1:1234" },
  { kind: "vllm", label: "vLLM", name: "vLLM", baseUrl: "http://127.0.0.1:8000" },
  {
    kind: "generic",
    label: "Other server (OpenAI-compatible)",
    name: "Other server",
    baseUrl: "https://example.com/v1",
  },
];

export const ENDPOINT_CHOICES: EndpointChoice[] = [...COMPANY_CHOICES, ...SERVER_CHOICES];

/** The choice for `kind`; an unknown kind (a newer app's) counts as "Other server". */
export function choiceFor(kind: ProviderKind): EndpointChoice {
  return ENDPOINT_CHOICES.find((c) => c.kind === kind) ?? SERVER_CHOICES[SERVER_CHOICES.length - 1]!;
}

/** The context sizes an endpoint can be held to (Automatic is `null`). */
export const CONTEXT_CHOICES = [8_192, 16_384, 32_768, 65_536, 131_072, 200_000, 400_000, 1_000_000];

/** The form's name and address after switching from `from` to `to`: each one still as the
 *  last choice filled it in (or empty) takes the new choice's; one the user typed stays. */
export function switchChoice(
  fields: { name: string; baseUrl: string },
  from: ProviderKind,
  to: ProviderKind,
): { name: string; baseUrl: string } {
  const before = choiceFor(from);
  const after = choiceFor(to);
  const name = fields.name.trim() === "" || fields.name === before.name ? after.name : fields.name;
  const filled = fields.baseUrl.trim() === "" || ENDPOINT_CHOICES.some((c) => c.baseUrl === fields.baseUrl.trim());
  return { name, baseUrl: filled ? after.baseUrl : fields.baseUrl };
}

/** The host an address names ("api.anthropic.com"), or the address itself when it is not one. */
export function hostOf(baseUrl: string): string {
  try {
    return new URL(baseUrl).host || baseUrl;
  } catch {
    return baseUrl;
  }
}
