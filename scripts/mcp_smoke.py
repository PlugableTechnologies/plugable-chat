#!/usr/bin/env python3
"""A bare-bones MCP client for the brix-box tester guide: no model, no app, standard library only (macOS, Linux, Windows).

It talks to the box one of two ways and prints one PASS or FAIL line per step. With --url it POSTs MCP messages to the box's HTTP endpoint.
Without it, it launches whatever command you give after `--` and speaks MCP over that command's stdin and stdout (the SSH route).
Either way it sends the same JSON-RPC any MCP client sends, so a red result here is not the fault of Plugable Chat or Claude Code.
Calls the box's `ask` only with --ask, because that is the one slow step (minutes) and it uses the local model.

  python mcp_smoke.py --url http://192.168.1.24:8787/mcp
  python mcp_smoke.py --url "http://[fd7a:115c:a1e0::9a01:5bca]:8787/mcp" --ask "which skill covers Amazon SQP?"
  python mcp_smoke.py -- ssh -i ~/.ssh/brix-box-tester -o BatchMode=yes bernieplug@192.168.1.24 docker exec -i brix-box node /opt/brix-box/mcp/server.mjs
  python mcp_smoke.py -- uv run actions/brix-box/scripts/brix_box.py mcp --host spark-redmond

Copied from brix-cli (actions/brix-box/docs/testing-guide/mcp_smoke.py), which is the source of truth for what the box must answer; copy it again
when the box's tools or self-checks change, so testers get the same file from either repository.
"""
import argparse
import json
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

TOOLS = {"ask", "match_skills", "read_skill", "status", "selfcheck"}
CHECKS = {"identity", "bigquery", "readonly", "redaction", "model", "pi", "skills"}


class Server:
    def __init__(self, argv):
        # stderr is passed through: ssh prints its connection errors there and they are the first thing a tester needs.
        self.p = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=None, text=True, bufsize=1)
        self.lines, self.cv, self.next_id = [], threading.Condition(), 0
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for ln in self.p.stdout:
            with self.cv:
                self.lines.append(ln)
                self.cv.notify_all()
        with self.cv:
            self.lines.append(None)  # end of stream
            self.cv.notify_all()

    def send(self, obj):
        self.p.stdin.write(json.dumps(obj) + "\n")
        self.p.stdin.flush()

    def call(self, method, params=None, timeout=30):
        self.next_id += 1
        want = self.next_id
        self.send({"jsonrpc": "2.0", "id": want, "method": method, **({"params": params} if params is not None else {})})
        end = time.time() + timeout
        with self.cv:
            while True:
                while self.lines:
                    ln = self.lines.pop(0)
                    if ln is None:
                        raise RuntimeError("the server closed the connection (look for an ssh or docker message above)")
                    try:
                        msg = json.loads(ln)
                    except ValueError:
                        raise RuntimeError(f"the server printed something that is not JSON on stdout: {ln.strip()[:120]!r}")
                    if msg.get("id") == want:
                        return msg
                left = end - time.time()
                if left <= 0:
                    raise TimeoutError(f"no answer to {method} in {timeout}s")
                self.cv.wait(left)

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:
            self.p.kill()


class HttpServer:
    """The same call/send/close surface as Server, over HTTP POSTs to the box's /mcp endpoint. Sends no Origin header, as a native client does."""

    def __init__(self, url):
        self.url, self.next_id = url, 0

    def _post(self, obj, timeout):
        req = urllib.request.Request(self.url, data=json.dumps(obj).encode(), method="POST", headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                body = r.read()
                return r.status, (json.loads(body) if body else None)
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"the box answered HTTP {e.code}: {e.read().decode(errors='replace')[:200]}")
        except (urllib.error.URLError, OSError) as e:
            raise RuntimeError(f"cannot reach {self.url}: {getattr(e, 'reason', e)} (wrong network, Tailscale off, or the box is down)")

    def send(self, obj):
        self._post(obj, 30)

    def call(self, method, params=None, timeout=30):
        self.next_id += 1
        _, msg = self._post({"jsonrpc": "2.0", "id": self.next_id, "method": method, **({"params": params} if params is not None else {})}, timeout)
        if not msg or msg.get("id") != self.next_id:
            raise RuntimeError(f"unexpected reply to {method}: {str(msg)[:120]}")
        return msg

    def close(self):
        pass


def tool(srv, name, args=None, timeout=30):
    msg = srv.call("tools/call", {"name": name, "arguments": args or {}}, timeout)
    if "error" in msg:
        raise RuntimeError(msg["error"])
    res = msg["result"]
    return res.get("isError", False), res.get("structuredContent"), "\n".join(c.get("text", "") for c in res.get("content", []))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--ask", metavar="QUESTION", help="also put one question to the box's model (slow: minutes)")
    ap.add_argument("--ask-timeout", type=int, default=900)
    ap.add_argument("--url", help="the box's HTTP MCP endpoint, for example http://192.168.1.24:8787/mcp (instead of a launch command)")
    ap.add_argument("cmd", nargs=argparse.REMAINDER, help="-- then the command that launches the server (the SSH route)")
    a = ap.parse_args()
    cmd = a.cmd[1:] if a.cmd[:1] == ["--"] else a.cmd
    if not cmd and not a.url:
        ap.error("give --url, or the launch command after --")

    failed = []

    def step(name, fn):
        t0 = time.time()
        try:
            ok, detail = fn()
        except Exception as e:  # a step that cannot run is a FAIL with the reason, never a traceback
            ok, detail = False, str(e)
        print(f"{'PASS' if ok else 'FAIL'}  {name:<22} {detail}  ({time.time() - t0:.1f}s)", flush=True)
        if not ok:
            failed.append(name)
        return ok

    srv = HttpServer(a.url) if a.url else Server(cmd)

    def init():
        r = srv.call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "mcp_smoke", "version": "1"}}, 60)
        srv.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        info = r["result"]["serverInfo"]
        return info.get("name") == "brix-box", f"{info.get('name')} {info.get('version')}"

    def tools():
        names = {t["name"] for t in srv.call("tools/list")["result"]["tools"]}
        return names == TOOLS, "five tools: " + ", ".join(sorted(names)) if names == TOOLS else f"got {sorted(names)}, want {sorted(TOOLS)}"

    def status():
        err, sc, txt = tool(srv, "status")
        return (not err) and sc and sc["model"]["ok"], txt

    def selfcheck():
        err, sc, txt = tool(srv, "selfcheck", timeout=120)
        bad = [k for k, v in (sc or {}).get("checks", {}).items() if not v["ok"]]
        return (not err) and sc and sc["ok"] and set(sc["checks"]) == CHECKS, "seven checks ok" if not bad else "red: " + ", ".join(bad) + "\n" + txt

    def match():
        err, sc, txt = tool(srv, "match_skills", {"query": "amazon sqp search query performance"})
        return (not err) and any(s["name"] == "amazon-sqp" for s in sc["skills"]), f"{len(sc['skills'])} skills incl. amazon-sqp"

    def read():
        err, _, txt = tool(srv, "read_skill", {"name": "amazon-sqp", "max_chars": 400})
        return (not err) and len(txt) > 50, f"{len(txt)} characters of SKILL.md"

    def refused():
        # A skill the box must not have (restricted, redacted before the image was built): the answer must be an error or an absence, never content.
        err, _, txt = tool(srv, "read_skill", {"name": "action-trakstar", "max_chars": 400})
        return err or "not found" in txt.lower() or "no such" in txt.lower(), "restricted skill is absent" if err or txt else "empty"

    def ask():
        err, _, txt = tool(srv, "ask", {"question": a.ask, "timeout_s": a.ask_timeout}, timeout=a.ask_timeout + 30)
        return (not err) and len(txt.strip()) > 0, "\n" + txt.strip()

    if step("initialize", init):
        step("tools/list", tools)
        step("status", status)
        step("selfcheck", selfcheck)
        step("match_skills", match)
        step("read_skill", read)
        step("restricted skill absent", refused)
        if a.ask:
            step("ask", ask)
    srv.close()
    print("\nALL PASS" if not failed else "\nFAILED: " + ", ".join(failed))
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
