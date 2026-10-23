"""Put a question to a language model and get a structured answer back.

A model is named "openrouter:<model>" (paid per use, from OPENROUTER_API_KEY
in secrets.env) or "claude:<model>" (the Claude Code installed on this
machine, on its owner's subscription). Models are given no tools and no
access to anything: they are handed text, or a sound clip, and return JSON.
What they say is checked against the reference before it is believed.
"""

import base64
import json
import os
import shutil
import subprocess
import time
import urllib.error
import urllib.request

from . import config

OPENROUTER = "https://openrouter.ai/api/v1/chat/completions"
PATIENCE = 120  # seconds one answer may take in all; the slowest good one so far took 56
LONGEST = 8000  # tokens an answer may run to unless the asker sets less; a model stuck repeating itself is billed for every one


class Failed(Exception):
    """This question got no usable answer; the next may."""


class Unusable(Failed):
    """The model answered, and was paid for it, but not with what was asked for."""


class Unavailable(Exception):
    """The model cannot be used at all for now: no key, not signed in, out of budget."""


def unfence(text):
    """The JSON inside an answer, whether or not it is wrapped in a code fence or prose."""
    text = text.strip()
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end < start:
        raise Unusable("answer holds no JSON")
    try:
        return json.loads(text[start:end + 1])
    except ValueError as e:
        raise Unusable(f"answer is not valid JSON: {e}") from e


def read_by(response, deadline):
    """The whole answer. OpenRouter keeps the line busy while a request is stalled, so the socket's own timeout never ends one."""
    parts = []
    while chunk := response.read1(65536):
        parts.append(chunk)
        if time.monotonic() > deadline:
            raise TimeoutError(f"no answer within {PATIENCE} seconds")
    return b"".join(parts)


def ask_openrouter(key, model, system, prompt, schema, audio=None, most=None):
    """The answer as the model wrote it, with what it took. It is left unread here so that its cost is known even when it is unusable."""
    content = prompt
    if audio is not None:
        content = [{"type": "text", "text": prompt},
                   {"type": "input_audio", "input_audio": {"data": base64.b64encode(audio).decode("ascii"), "format": "mp3"}}]
    body = {
        "model": model,
        "messages": [{"role": "system", "content": system}, {"role": "user", "content": content}],
        "temperature": 0,
        "max_tokens": most or LONGEST,
        "response_format": {"type": "json_schema", "json_schema": {"name": "answer", "strict": True, "schema": schema}},
        "usage": {"include": True},
    }
    request = urllib.request.Request(OPENROUTER, data=json.dumps(body).encode(), headers={
        "Authorization": "Bearer " + key, "Content-Type": "application/json", "X-Title": "wax librarian"})
    deadline = time.monotonic() + PATIENCE
    try:
        with urllib.request.urlopen(request, timeout=PATIENCE) as response:
            data = json.loads(read_by(response, deadline))
    except urllib.error.HTTPError as e:
        detail = e.read(600).decode("utf-8", "replace")
        if e.code in (401, 402, 403):  # bad key, no credit, not allowed
            raise Unavailable(f"OpenRouter answered {e.code}: {detail}") from e
        raise Failed(f"OpenRouter answered {e.code}: {detail}") from e
    except (urllib.error.URLError, TimeoutError, ConnectionError, ValueError) as e:
        raise Failed(f"OpenRouter: {e}") from e
    if data.get("error"):
        raise Failed(f"OpenRouter: {str(data['error'])[:300]}")
    try:
        text = data["choices"][0]["message"]["content"]
    except (KeyError, IndexError, TypeError) as e:
        raise Failed("OpenRouter gave no answer") from e
    usage = data.get("usage") or {}
    return text or "", usage.get("prompt_tokens"), usage.get("completion_tokens"), usage.get("cost") or 0.0


def ask_claude(model, system, prompt, schema):
    program = shutil.which("claude")
    if not program:
        raise Unavailable("Claude Code is not installed here")
    workdir = config.path("agent")  # an empty folder, so it has no project to read
    os.makedirs(workdir, exist_ok=True)
    # The subscription only: never a metered Anthropic key, whatever the environment holds.
    env = {name: value for name, value in os.environ.items() if not name.startswith(("ANTHROPIC_", "CLAUDE"))}
    command = [program, "-p", "--tools", "", "--strict-mcp-config", "--no-session-persistence", "--model", model,
               "--output-format", "json", "--system-prompt", system, "--json-schema", json.dumps(schema)]
    try:
        out = subprocess.run(command, input=prompt, capture_output=True, text=True, timeout=600, cwd=workdir, env=env)
        data = json.loads(out.stdout)
    except subprocess.TimeoutExpired as e:
        raise Failed("Claude took too long") from e
    except ValueError as e:
        raise Failed(f"Claude gave no JSON: {(out.stdout or out.stderr)[:300]}") from e
    if data.get("is_error"):
        result = str(data.get("result"))[:300]
        if "authenticat" in result.lower() or "login" in result.lower() or "limit" in result.lower():
            raise Unavailable(f"Claude: {result}")
        raise Failed(f"Claude: {result}")
    answer = data.get("structured_output")
    if not isinstance(answer, dict):
        answer = unfence(str(data.get("result") or ""))
    usage = data.get("usage") or {}
    return answer, usage.get("input_tokens"), usage.get("output_tokens"), 0.0


class Models:
    """The models a stage may ask, with every question logged and paid ones held to a budget."""

    def __init__(self, db, stage, budget=None):
        self.db, self.stage = db, stage
        self.keys = config.secrets()
        settings = config.load()
        self.budget = settings.get(f"{stage}_budget_usd", settings["budget_usd"]) if budget is None else budget
        self.claude_cap, self.claude_calls = settings["claude_questions"], 0
        self.spent, self.calls, self.down = 0.0, 0, {}
        self.unanswered = 0  # questions in a row that got no usable answer

    def usable(self, name):
        """Whether a model can be asked at all, as far as can be told without asking."""
        if name in self.down:
            return False
        backend = name.partition(":")[0]
        if backend == "openrouter":
            return bool(self.keys.get("OPENROUTER_API_KEY")) and self.spent < self.budget
        return backend == "claude" and bool(shutil.which("claude")) and self.claude_calls < self.claude_cap

    def why_not(self, name):
        if name in self.down:
            return self.down[name]
        if name.startswith("openrouter:"):
            return "no OPENROUTER_API_KEY in secrets.env" if not self.keys.get("OPENROUTER_API_KEY") else "this run's budget is spent"
        return "this run's share of the subscription is used" if shutil.which("claude") else "Claude Code is not installed here"

    def ask(self, name, system, prompt, schema, audio=None, most=None):
        """The model's answer as a dict, held to `most` tokens. Raises Failed for this question, Unavailable for the model."""
        backend, _, model = name.partition(":")
        if not self.usable(name):
            raise Unavailable(self.why_not(name))
        began, tokens_in, tokens_out, usd, error, answered = time.time(), None, None, 0.0, None, True
        try:
            if backend == "openrouter":
                text, tokens_in, tokens_out, usd = ask_openrouter(self.keys["OPENROUTER_API_KEY"], model, system, prompt, schema, audio, most)
                answer = unfence(text)
            else:
                if audio is not None:
                    raise Unavailable("Claude Code cannot be given sound")
                self.claude_calls += 1
                answer, tokens_in, tokens_out, usd = ask_claude(model, system, prompt, schema)
            return answer
        except Unavailable as e:
            self.down[name] = error = str(e)
            answered = False
            raise
        except Failed as e:
            error, answered = str(e), isinstance(e, Unusable)
            raise
        finally:
            self.spent += usd
            self.calls += 1
            self.unanswered = 0 if answered else self.unanswered + 1
            self.db.execute("INSERT INTO asked (at, stage, model, tokens_in, tokens_out, usd, seconds, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                            (int(began), self.stage, name, tokens_in, tokens_out, usd, round(time.time() - began, 1), error))
            self.db.commit()
