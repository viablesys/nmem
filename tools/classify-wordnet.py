#!/usr/bin/env python3
"""
WordNet-bridged classification: LLM outputs a free-form verb,
WordNet maps it to plan/build via synset distance.

Usage:
    python3 tools/classify-wordnet.py --corpus tools/corpus-plan-build-100.json \
        --endpoint http://10.0.0.148:1234/v1/chat/completions \
        --model qwen/qwen3-coder-30b

Requires: nltk with wordnet corpus, LM Studio endpoint.
"""

import argparse
import json
import re
import sys
import time
import urllib.request

import nltk
nltk.download("wordnet", quiet=True)
nltk.download("omw-1.4", quiet=True)
from nltk.corpus import wordnet as wn

DEFAULT_ENDPOINT = "http://10.0.0.148:1234/v1/chat/completions"
DEFAULT_MODEL = "qwen/qwen3-coder-30b"

# Anchor synsets for each pole
PLAN_ANCHORS = [
    wn.synset("plan.v.01"),       # have the will and intention
    wn.synset("plan.v.02"),       # make plans for something
    wn.synset("investigate.v.01"),# investigate scientifically
    wn.synset("analyze.v.01"),    # consider in detail
    wn.synset("evaluate.v.02"),   # form a critical opinion of
]

BUILD_ANCHORS = [
    wn.synset("construct.v.01"),  # make by combining materials
    wn.synset("implement.v.01"),  # apply in a manner consistent with its purpose
    wn.synset("execute.v.01"),    # carry out
    wn.synset("repair.v.01"),     # restore by replacing a part or putting together what is torn
    wn.synset("commit.v.01"),     # perform an act
]

SYSTEM_PROMPT = """You describe coding session activity with a SINGLE action verb.

Given text from a coding session, respond with ONLY one verb that best describes what is happening.
Examples: investigating, implementing, committing, analyzing, debugging, writing, reviewing, deploying, designing, refactoring, testing, exploring, deciding, fixing, creating, diagnosing, observing, renaming, configuring

Return ONLY the verb. One word. No explanation."""

USER_TEMPLATE = """What single verb describes this activity?

---
{text}
---

One verb only."""


def call_llm(endpoint, model, system, user, timeout=30):
    body = json.dumps({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.0,
        "max_tokens": 16,
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
            # Strip to single word
            text = text.strip().strip('"').strip("'").strip(".").lower()
            # Take first word if multiple
            text = re.split(r"[\s,;]+", text)[0]
            return text
    except Exception as e:
        return None


def wordnet_score(verb):
    """Score a verb against plan/build anchor synsets.
    Returns (plan_score, build_score, best_synset_used)."""
    synsets = wn.synsets(verb, pos=wn.VERB)
    if not synsets:
        # Try lemmatizing
        from nltk.stem import WordNetLemmatizer
        wnl = WordNetLemmatizer()
        lemma = wnl.lemmatize(verb, "v")
        synsets = wn.synsets(lemma, pos=wn.VERB)

    if not synsets:
        return 0.0, 0.0, None

    best_plan = 0.0
    best_build = 0.0

    for s in synsets:
        for anchor in PLAN_ANCHORS:
            sim = s.path_similarity(anchor)
            if sim and sim > best_plan:
                best_plan = sim

        for anchor in BUILD_ANCHORS:
            sim = s.path_similarity(anchor)
            if sim and sim > best_build:
                best_build = sim

    return best_plan, best_build, synsets[0].name()


def eval_corpus(endpoint, model, corpus_path):
    with open(corpus_path) as f:
        corpus = json.load(f)

    correct = 0
    total = 0
    per_type = {"plan": [0, 0], "build": [0, 0]}
    confusion = {}
    total_ms = 0
    errors = 0
    no_wn = 0
    results = []
    verb_freq = {}

    print(f"Evaluating {len(corpus)} entries with {model} + WordNet")
    print(f"Endpoint: {endpoint}")
    print(f"Plan anchors: {[s.name() for s in PLAN_ANCHORS]}")
    print(f"Build anchors: {[s.name() for s in BUILD_ANCHORS]}")
    print()

    for entry in corpus:
        text = entry["text"][:500]
        expected = entry["type"]
        user = USER_TEMPLATE.format(text=text)

        t0 = time.monotonic()
        verb = call_llm(endpoint, model, SYSTEM_PROMPT, user)
        elapsed_ms = round((time.monotonic() - t0) * 1000)
        total_ms += elapsed_ms

        if not verb:
            errors += 1
            preview = text[:55].replace("\n", " ")
            print(f"  [E] {preview:<55} ERROR: no response")
            results.append({"id": entry["id"], "error": "no response", "latency_ms": elapsed_ms})
            continue

        verb_freq[verb] = verb_freq.get(verb, 0) + 1
        plan_s, build_s, synset = wordnet_score(verb)

        if plan_s == 0 and build_s == 0:
            no_wn += 1
            predicted = "?"
        elif build_s > plan_s:
            predicted = "build"
        elif plan_s > build_s:
            predicted = "plan"
        else:
            predicted = "?"  # tie

        total += 1
        per_type[expected][1] += 1
        match = predicted == expected
        if match:
            correct += 1
            per_type[expected][0] += 1

        key = (expected, predicted)
        confusion[key] = confusion.get(key, 0) + 1

        preview = text[:45].replace("\n", " ")
        mark = "+" if match else "X"
        print(f"  [{mark}] {preview:<45} verb={verb:<15} p={plan_s:.3f} b={build_s:.3f} → {predicted:<6} exp={expected}  {elapsed_ms}ms")

        results.append({
            "id": entry["id"],
            "expected": expected,
            "predicted": predicted,
            "verb": verb,
            "plan_score": round(plan_s, 4),
            "build_score": round(build_s, 4),
            "synset": synset,
            "match": match,
            "latency_ms": elapsed_ms,
        })

    # Summary
    print(f"\n{'='*70}")
    if total > 0:
        print(f"ACCURACY: {correct}/{total} = {correct/total*100:.1f}%")
        print(f"Avg latency: {total_ms // len(corpus)}ms")
        if errors:
            print(f"Errors: {errors}")
        if no_wn:
            print(f"No WordNet match: {no_wn}")

        print(f"\nPer-type:")
        for t in ["plan", "build"]:
            c, n = per_type[t]
            if n > 0:
                print(f"  {t:<6} {c}/{n} = {c/n*100:.0f}%")

        errs = {k: v for k, v in confusion.items() if k[0] != k[1]}
        if errs:
            print(f"\nConfusions:")
            for (exp, got), count in sorted(errs.items(), key=lambda x: -x[1]):
                print(f"  {exp} → {got}: {count}")

        print(f"\nVerb frequency (top 20):")
        for verb, count in sorted(verb_freq.items(), key=lambda x: -x[1])[:20]:
            ps, bs, syn = wordnet_score(verb)
            direction = "PLAN" if ps > bs else "BUILD" if bs > ps else "TIE"
            print(f"  {verb:<20} {count:>3}x  p={ps:.3f} b={bs:.3f} → {direction}")

    out_path = "/tmp/nmem-wordnet-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


def main():
    parser = argparse.ArgumentParser(description="WordNet-bridged LLM classification")
    parser.add_argument("--corpus", required=True, help="Labeled corpus JSON")
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT)
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument("--text", help="Test single text")
    args = parser.parse_args()

    if args.text:
        user = USER_TEMPLATE.format(text=args.text)
        verb = call_llm(args.endpoint, args.model, SYSTEM_PROMPT, user)
        if verb:
            ps, bs, syn = wordnet_score(verb)
            direction = "plan" if ps > bs else "build" if bs > ps else "tie"
            print(f"verb={verb}  synset={syn}  plan={ps:.3f}  build={bs:.3f}  → {direction}")
        else:
            print("Error: no response")
        return

    eval_corpus(args.endpoint, args.model, args.corpus)


if __name__ == "__main__":
    main()
