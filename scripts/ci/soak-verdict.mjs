#!/usr/bin/env node
// Soak verdict (infra/aws-gpu/soak-run.ps1): judges the turn records of a soak run against the harness's own
// timeline of network flaps, app launches, kills and observed records. Pure logic is exported for the node test.
//
//   node scripts/ci/soak-verdict.mjs --replies replies-all.jsonl --events events.jsonl [--crashes crashes.json] [--prompts N]
//
// events.jsonl: one JSON object per line, t = epoch milliseconds:
//   {t, event:"launch"} {t, event:"flap_start"} {t, event:"flap_end"} {t, event:"kill"} {t, event:"record", prompt_index}
// crashes.json: array of epoch-ms (or ISO) times of Application Error events naming the app, already limited to this run.
// Prints {"failures":[...],"notes":[...],"stats":{...}} and exits 1 when there are failures.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// A flap that starts or ends within this long of a turn still counts as overlapping it: the record time is
// observed by polling, so it can lag the true end by a few seconds.
export const FLAP_MARGIN_MS = 5000;
// An Application Error this close to the harness's deliberate kill is the kill, not a crash.
export const KILL_WINDOW_MS = 30000;

const isDrop = (r) => r.outcome === 'error' && String(r.detail ?? '').startsWith('network_dropped');

/** Flap windows [start, end] from flap_start/flap_end events; an unmatched start runs to the end of the run. */
export function flapWindows(events) {
  const wins = [];
  let open = null;
  for (const e of [...events].sort((a, b) => a.t - b.t)) {
    if (e.event === 'flap_start' && open === null) open = e.t;
    else if (e.event === 'flap_end' && open !== null) { wins.push([open, e.t]); open = null; }
  }
  if (open !== null) wins.push([open, Number.POSITIVE_INFINITY]);
  return wins;
}

/** When the turn began and ended, from the observed record time and the turn's own duration; null when unknown. */
export function turnInterval(record, events) {
  const seen = events.find((e) => e.event === 'record' && Number(e.prompt_index) === Number(record.prompt_index));
  if (!seen) return null;
  const duration = Number(record.duration_ms) || 0;
  return { begin: seen.t - duration, end: seen.t };
}

const overlaps = (iv, wins, margin) => wins.some(([s, e]) => iv.begin - margin <= e && iv.end + margin >= s);

/**
 * @param records the renumbered reply records [{ prompt_index, outcome, detail, duration_ms }]
 * @param events  the harness timeline
 * @param opts    { prompts, crashes: [ms|iso], flapMarginMs }
 */
export function verdictFor(records, events, opts = {}) {
  const margin = opts.flapMarginMs ?? FLAP_MARGIN_MS;
  const failures = [];
  const notes = [];
  const wins = flapWindows(events);
  const sorted = [...records].sort((a, b) => Number(a.prompt_index) - Number(b.prompt_index));
  let expectedDrops = 0;

  for (const r of sorted) {
    if (r.outcome === 'ok') continue;
    const iv = turnInterval(r, events);
    const flapped = iv ? overlaps(iv, wins, margin) : wins.length > 0;
    if (isDrop(r)) {
      if (flapped) { expectedDrops++; notes.push(`turn ${r.prompt_index} ended network_dropped while the harness had the network down: expected`); }
      else failures.push(`turn ${r.prompt_index} ended network_dropped but no flap overlapped it: the app saw a drop the harness did not cause`);
    } else if (flapped) {
      failures.push(`turn ${r.prompt_index} ended "${r.outcome}" (${String(r.detail ?? '').slice(0, 80)}) during a flap, but not as a clean network_dropped`);
    } else {
      failures.push(`turn ${r.prompt_index} began with the network up, no flap overlapped it, and it failed "${r.outcome}": ${String(r.detail ?? '').slice(0, 80)}`);
    }
  }

  // Two failures in a row on turns that no flap touched are called out as the app staying broken.
  const clean = (r) => { const iv = turnInterval(r, events); return !(iv ? overlaps(iv, wins, margin) : wins.length > 0); };
  for (let i = 0; i + 1 < sorted.length; i++) {
    if (sorted[i].outcome !== 'ok' && sorted[i + 1].outcome !== 'ok' && clean(sorted[i]) && clean(sorted[i + 1])) {
      failures.push(`turns ${sorted[i].prompt_index} and ${sorted[i + 1].prompt_index} both failed with the network up: the app did not recover`);
    }
  }

  if (opts.prompts != null && sorted.length && Number(sorted[sorted.length - 1].prompt_index) < opts.prompts - 1) {
    failures.push(`the run stopped at turn ${sorted[sorted.length - 1].prompt_index}, though turns up to ${opts.prompts - 1} were due`);
  }
  if (wins.length > 0 && expectedDrops === 0) failures.push(`${wins.length} flap(s) never landed on a running turn: the soak did not exercise a drop (shorten the flap interval)`);

  const kills = events.filter((e) => e.event === 'kill').map((e) => e.t);
  const crashes = (opts.crashes ?? []).map((c) => (typeof c === 'number' ? c : Date.parse(c))).filter((c) => !Number.isNaN(c));
  const real = crashes.filter((c) => !kills.some((k) => Math.abs(c - k) <= KILL_WINDOW_MS));
  if (crashes.length > real.length) notes.push(`${crashes.length - real.length} Application Error event(s) within ${KILL_WINDOW_MS / 1000} s of the harness's own kill ignored`);
  if (real.length) failures.push(`${real.length} Application Error event(s) naming plugable-chat during this soak run, outside the harness's own kill`);

  return { failures, notes, stats: { records: sorted.length, flaps: wins.length, expectedDrops, crashes: real.length } };
}

/** The earliest time the next flap may start: never in the first `settleMs` after a launch. */
export function nextFlapNotBefore(launchAt, wantedAt, settleMs = 90000) {
  return Math.max(wantedAt, launchAt + settleMs);
}

function readJsonl(path) {
  return readFileSync(path, 'utf8').split(/\r?\n/).filter((l) => l.trim()).map((l) => JSON.parse(l.replace(/^﻿/, '')));
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const get = (f) => (args.includes(f) ? args[args.indexOf(f) + 1] : undefined);
  if (!get('--replies') || !get('--events')) {
    console.error('usage: soak-verdict.mjs --replies <replies.jsonl> --events <events.jsonl> [--crashes <crashes.json>] [--prompts N]');
    process.exit(2);
  }
  const crashes = get('--crashes') ? JSON.parse(readFileSync(get('--crashes'), 'utf8').replace(/^﻿/, '') || '[]') : [];
  const out = verdictFor(readJsonl(get('--replies')), readJsonl(get('--events')), { prompts: get('--prompts') ? Number(get('--prompts')) : undefined, crashes: [].concat(crashes) });
  console.log(JSON.stringify(out));
  process.exit(out.failures.length ? 1 : 0);
}
