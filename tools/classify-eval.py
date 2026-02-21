#!/usr/bin/env python3
"""
Evaluate think/act classification of prompts and thinking blocks.

Usage:
    python3 tools/classify-eval.py                          # classify sample prompts from DB
    python3 tools/classify-eval.py --text "fix the auth bug"  # classify a single string
    python3 tools/classify-eval.py --corpus tools/corpus.json  # eval against labeled corpus
    python3 tools/classify-eval.py --dump                    # dump raw prompts from DB (no LLM)
    python3 tools/classify-eval.py --model MODEL             # use specific model
    python3 tools/classify-eval.py --endpoint URL            # use specific endpoint
    python3 tools/classify-eval.py --model-file models/think-act.json --corpus tools/corpus.json

Requires: LM Studio running on localhost:1234 with a model loaded.
         --model-file mode requires numpy (no LLM needed).
"""

import argparse
import json
import os
import sqlite3
import sys
import time
import urllib.request

DEFAULT_ENDPOINT = "http://localhost:1234/v1/chat/completions"
DEFAULT_MODEL = "ibm/granite-4-h-tiny"

SYSTEM_PROMPT = """You classify coding session text as either "think" or "act".

- think: figuring out what to do — investigating, exploring, deciding, reviewing, diagnosing, asking questions, making observations, evaluating trade-offs
- act: doing the thing — implementing, executing instructions, committing, writing code/docs, fixing bugs, creating files, running tests

Return ONLY a JSON object: {"type": "think" or "act", "confidence": 0.0-1.0}
No explanation, no markdown."""

USER_TEMPLATE = """Classify this {source} text:

---
{text}
---

Return JSON only."""


def call_llm(endpoint, model, system, user, timeout=30):
    body = json.dumps({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.0,
        "max_tokens": 64,
    }).encode()

    req = urllib.request.Request(
        endpoint,
        data=body,
        headers={"Content-Type": "application/json"},
    )

    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.loads(resp.read())
            text = data["choices"][0]["message"]["content"]
            text = text.strip()
            if text.startswith("```"):
                lines = text.split("\n")
                lines = [l for l in lines if not l.strip().startswith("```")]
                text = "\n".join(lines).strip()
            return json.loads(text)
    except Exception as e:
        return {"error": str(e)}


## --- Exported JSON model evaluation (--model-file) ---

class TfidfModel:
    """Evaluate an exported TF-IDF + LogReg JSON model (mirrors Rust classifier)."""

    def __init__(self, model_path):
        import math
        self.math = math

        with open(model_path) as f:
            data = json.load(f)

        self.classes = data["classes"]
        self.bias = data["bias"]

        self.word_vocab = data["word"]["vocabulary"]
        self.word_idf = data["word"]["idf"]
        self.word_weights = data["word"]["weights"]
        self.word_ngram_range = tuple(data["word"]["ngram_range"])
        self.word_binary = data["word"].get("binary", False)
        self.word_sublinear = data["word"].get("sublinear_tf", True)

        self.char_vocab = data["char"]["vocabulary"]
        self.char_idf = data["char"]["idf"]
        self.char_weights = data["char"]["weights"]
        self.char_ngram_range = tuple(data["char"]["ngram_range"])
        self.char_sublinear = data["char"].get("sublinear_tf", True)

    def _word_tokenize(self, text):
        """Split on whitespace + punctuation boundaries, lowercase."""
        import re
        tokens = re.findall(r'\b\w+\b', text.lower())
        return tokens

    def _word_ngrams(self, tokens):
        """Generate word n-grams."""
        lo, hi = self.word_ngram_range
        ngrams = {}
        for n in range(lo, hi + 1):
            for i in range(len(tokens) - n + 1):
                gram = " ".join(tokens[i:i+n])
                ngrams[gram] = ngrams.get(gram, 0) + 1
        return ngrams

    def _char_ngrams(self, text):
        """Generate char_wb n-grams (whitespace-bounded)."""
        lo, hi = self.char_ngram_range
        text_lower = text.lower()
        # char_wb: pad each word with spaces, then extract char n-grams
        import re
        words = re.findall(r'\S+', text_lower)
        ngrams = {}
        for word in words:
            padded = f" {word} "
            for n in range(lo, hi + 1):
                for i in range(len(padded) - n + 1):
                    gram = padded[i:i+n]
                    ngrams[gram] = ngrams.get(gram, 0) + 1
        return ngrams

    def _tfidf_score(self, ngrams, vocab, idf, weights, binary, sublinear):
        """Compute TF-IDF dot product with weights."""
        score = 0.0
        # Compute L2 norm for normalization
        tfidf_values = {}
        for gram, count in ngrams.items():
            if gram in vocab:
                idx = vocab[gram]
                tf = 1.0 if binary else (self.math.log(count + 1) if sublinear else float(count))
                tfidf_values[idx] = tf * idf[idx]

        # L2 normalize
        norm = self.math.sqrt(sum(v * v for v in tfidf_values.values()))
        if norm == 0:
            return 0.0

        for idx, val in tfidf_values.items():
            score += (val / norm) * weights[idx]

        return score

    def classify(self, text):
        """Classify text, return (label, confidence)."""
        words = self._word_tokenize(text)
        word_ngrams = self._word_ngrams(words)
        char_ngrams = self._char_ngrams(text)

        word_score = self._tfidf_score(
            word_ngrams, self.word_vocab, self.word_idf,
            self.word_weights, self.word_binary, self.word_sublinear,
        )
        char_score = self._tfidf_score(
            char_ngrams, self.char_vocab, self.char_idf,
            self.char_weights, False, self.char_sublinear,
        )

        raw = word_score + char_score + self.bias
        prob = 1.0 / (1.0 + self.math.exp(-raw))

        # classes[1] is the positive class (plan), classes[0] is build
        if prob >= 0.5:
            return self.classes[1], prob
        else:
            return self.classes[0], 1.0 - prob


def eval_model_file(model_path, corpus_path):
    """Evaluate exported JSON model against a labeled corpus."""
    model = TfidfModel(model_path)

    with open(corpus_path) as f:
        corpus = json.load(f)

    correct = 0
    total = len(corpus)
    per_type = {"think": [0, 0], "act": [0, 0]}
    confusion = {}
    results = []

    print(f"Evaluating {total} entries with exported model: {model_path}")
    print(f"Classes: {model.classes}")
    print()

    for entry in corpus:
        text = entry["text"][:500]
        expected = entry["type"]

        predicted, conf = model.classify(text)

        per_type[expected][1] += 1
        match = predicted == expected
        if match:
            correct += 1
            per_type[expected][0] += 1

        key = (expected, predicted)
        confusion[key] = confusion.get(key, 0) + 1

        preview = text[:55].replace("\n", " ")
        mark = "+" if match else "X"
        print(f"  [{mark}] {preview:<55} exp={expected:<6} got={predicted:<6} conf={conf:.3f}")

        results.append({
            "id": entry["id"],
            "expected": expected,
            "predicted": predicted,
            "match": match,
            "confidence": round(conf, 4),
        })

    print(f"\n{'='*60}")
    print(f"ACCURACY: {correct}/{total} = {correct/total*100:.1f}%")

    print(f"\nPer-type:")
    for t in ["think", "act"]:
        c, n = per_type[t]
        if n > 0:
            precision = c / max(1, sum(1 for r in results if r["predicted"] == t))
            recall = c / n
            f1 = 2 * precision * recall / max(precision + recall, 1e-9)
            print(f"  {t:<6} {c}/{n} = {recall*100:.0f}% recall, {precision*100:.0f}% precision, F1={f1:.3f}")

    errs = {k: v for k, v in confusion.items() if k[0] != k[1]}
    if errs:
        print(f"\nConfusions:")
        for (exp, got), count in sorted(errs.items(), key=lambda x: -x[1]):
            print(f"  {exp} → {got}: {count}")

    out_path = "/tmp/nmem-model-file-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


def get_db_path():
    return os.path.expanduser("~/.nmem/nmem.db")


def open_db():
    db_path = get_db_path()
    if not os.path.exists(db_path):
        print(f"DB not found: {db_path}", file=sys.stderr)
        sys.exit(1)
    conn = sqlite3.connect(db_path)
    key_file = os.path.expanduser("~/.nmem/key")
    if os.path.exists(key_file):
        with open(key_file) as f:
            key = f.read().strip()
        conn.execute(f"PRAGMA key = '{key}'")
    return conn


def dump_prompts(conn, limit=50):
    cur = conn.execute(
        """SELECT p.id, p.source, p.timestamp, p.content, s.project
           FROM prompts p
           JOIN sessions s ON p.session_id = s.id
           ORDER BY p.timestamp DESC
           LIMIT ?""",
        (limit,),
    )
    rows = cur.fetchall()
    print(f"{'ID':>6} {'Source':>8} {'Project':>12} {'Length':>6}  Preview")
    print("-" * 80)
    for row in rows:
        pid, source, ts, content, project = row
        preview = content[:80].replace("\n", " ")
        print(f"{pid:>6} {source:>8} {project or '?':>12} {len(content):>6}  {preview}")
    return rows


def sample_prompts(conn, limit=20):
    cur = conn.execute(
        """SELECT p.id, p.source, p.content, s.project
           FROM prompts p
           JOIN sessions s ON p.session_id = s.id
           WHERE LENGTH(p.content) > 10
           ORDER BY p.timestamp DESC
           LIMIT ?""",
        (limit,),
    )
    return cur.fetchall()


def eval_corpus(endpoint, model, corpus_path):
    with open(corpus_path) as f:
        corpus = json.load(f)

    correct = 0
    total = 0
    per_type = {"think": [0, 0], "act": [0, 0]}  # [correct, total]
    confusion = {}
    total_ms = 0
    errors = 0
    results = []

    print(f"Evaluating {len(corpus)} entries with {model}...")
    print(f"Endpoint: {endpoint}")
    print()

    for entry in corpus:
        text = entry["text"][:500]
        expected = entry["type"]
        # Source is folded into text as [user] or [agent] prefix
        source_label = "agent reasoning" if text.startswith("[agent]") else "user prompt"
        user = USER_TEMPLATE.format(source=source_label, text=text)

        t0 = time.monotonic()
        result = call_llm(endpoint, model, SYSTEM_PROMPT, user)
        elapsed_ms = round((time.monotonic() - t0) * 1000)
        total_ms += elapsed_ms

        err = result.get("error")
        if err:
            errors += 1
            preview = text[:55].replace("\n", " ")
            print(f"  [E] {preview:<55} ERROR: {err}")
            results.append({"id": entry["id"], "error": err, "latency_ms": elapsed_ms})
            continue

        predicted = result.get("type", "?")
        conf = result.get("confidence", 0)
        total += 1
        per_type[expected][1] += 1
        match = predicted == expected
        if match:
            correct += 1
            per_type[expected][0] += 1

        key = (expected, predicted)
        confusion[key] = confusion.get(key, 0) + 1

        preview = text[:55].replace("\n", " ")
        mark = "+" if match else "X"
        print(f"  [{mark}] {preview:<55} exp={expected:<6} got={predicted:<6} conf={conf:.1f}  {elapsed_ms}ms")

        results.append({
            "id": entry["id"],
            "expected": expected,
            "predicted": predicted,
            "match": match,
            "confidence": conf,
            "latency_ms": elapsed_ms,
        })

    # Summary
    print(f"\n{'='*60}")
    if total > 0:
        print(f"ACCURACY: {correct}/{total} = {correct/total*100:.1f}%")
        print(f"Avg latency: {total_ms // len(corpus)}ms")
        if errors:
            print(f"Errors: {errors}")

        print(f"\nPer-type:")
        for t in ["think", "act"]:
            c, n = per_type[t]
            if n > 0:
                print(f"  {t:<6} {c}/{n} = {c/n*100:.0f}%")

        errs = {k: v for k, v in confusion.items() if k[0] != k[1]}
        if errs:
            print(f"\nConfusions:")
            for (exp, got), count in sorted(errs.items(), key=lambda x: -x[1]):
                print(f"  {exp} → {got}: {count}")
    else:
        print("No valid results (all errors)")

    out_path = "/tmp/nmem-granite-corpus-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


def main():
    parser = argparse.ArgumentParser(description="Evaluate LLM prompt classification")
    parser.add_argument("--text", help="Classify a single text string")
    parser.add_argument("--corpus", help="Evaluate against a labeled corpus JSON file")
    parser.add_argument("--model-file", help="Evaluate exported JSON model (no LLM needed)")
    parser.add_argument("--dump", action="store_true", help="Dump raw prompts (no LLM)")
    parser.add_argument("--limit", type=int, default=20, help="Number of prompts to sample")
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--source", default="user prompt", help="Source label for --text mode")
    args = parser.parse_args()

    if args.model_file:
        if args.corpus:
            eval_model_file(args.model_file, args.corpus)
        elif args.text:
            model = TfidfModel(args.model_file)
            label, conf = model.classify(args.text)
            print(json.dumps({"type": label, "confidence": round(conf, 4)}))
        else:
            print("--model-file requires --corpus or --text", file=sys.stderr)
            sys.exit(1)
        return

    if args.corpus:
        eval_corpus(args.endpoint, args.model, args.corpus)
        return

    if args.text:
        source_label = args.source
        user = USER_TEMPLATE.format(source=source_label, text=args.text[:500])
        t0 = time.monotonic()
        result = call_llm(args.endpoint, args.model, SYSTEM_PROMPT, user)
        result["latency_ms"] = round((time.monotonic() - t0) * 1000)
        print(json.dumps(result, indent=2))
        return

    if args.dump:
        conn = open_db()
        dump_prompts(conn, args.limit)
        conn.close()
        return

    # Batch mode from DB
    conn = open_db()
    rows = sample_prompts(conn, args.limit)
    conn.close()

    if not rows:
        print("No prompts found in DB", file=sys.stderr)
        sys.exit(1)

    results = []
    total_latency = 0

    print(f"Classifying {len(rows)} prompts with {args.model}...")
    print(f"Endpoint: {args.endpoint}")
    print()

    for pid, source, content, project in rows:
        preview = content[:60].replace("\n", " ")
        source_label = "agent reasoning" if source == "agent" else "user prompt"
        user = USER_TEMPLATE.format(source=source_label, text=content[:500])

        start = time.monotonic()
        result = call_llm(args.endpoint, args.model, SYSTEM_PROMPT, user)
        elapsed = time.monotonic() - start
        latency = round(elapsed * 1000)
        total_latency += latency

        record = {
            "id": pid,
            "source": source,
            "project": project,
            "preview": preview,
            "classification": result,
            "latency_ms": latency,
        }
        results.append(record)

        typ = result.get("type", "?")
        conf = result.get("confidence", 0)
        err = result.get("error")

        if err:
            print(f"  [{source:>5}] {preview:<60} ERROR: {err}")
        else:
            print(f"  [{source:>5}] {preview:<60} → {typ:<6} ({conf:.1f}) {latency}ms")

    print()
    print(f"--- Summary ---")
    print(f"Total: {len(results)} prompts")
    print(f"Avg latency: {total_latency // len(results)}ms")

    types = {}
    for r in results:
        t = r["classification"].get("type", "error")
        types[t] = types.get(t, 0) + 1
    print(f"Type distribution: {json.dumps(types)}")

    out_path = "/tmp/nmem-classify-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


if __name__ == "__main__":
    main()
