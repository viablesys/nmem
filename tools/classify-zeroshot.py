#!/usr/bin/env python3
"""
Zero-shot prompt classification using GLiClass + ModernBERT.
No training needed — just category descriptions.

Usage:
    python3 tools/classify-zeroshot.py                          # batch from DB
    python3 tools/classify-zeroshot.py --text "fix the bug"     # single text
    python3 tools/classify-zeroshot.py --corpus tools/corpus.json  # eval against labeled corpus
    python3 tools/classify-zeroshot.py --device cpu              # force CPU
"""

import argparse
import json
import os
import sqlite3
import sys
import time

MODEL_ID = "knowledgator/gliclass-modern-base-v2.0-init"

# Work types — what cognitive mode is active
TYPE_LABELS = ["plan", "build"]

# Understanding concepts — what kind of knowledge is forming
CONCEPT_LABELS = ["how-it-works", "why-it-exists", "problem-solution", "gotcha", "pattern", "trade-off", "unresolved"]


def load_pipeline(device):
    from gliclass import GLiClassModel, ZeroShotClassificationPipeline
    from transformers import AutoTokenizer

    print(f"Loading {MODEL_ID} on {device}...", file=sys.stderr)
    t0 = time.monotonic()
    model = GLiClassModel.from_pretrained(MODEL_ID)
    tokenizer = AutoTokenizer.from_pretrained(MODEL_ID)
    pipeline = ZeroShotClassificationPipeline(
        model, tokenizer,
        classification_type='multi-label',
        device=device,
    )
    print(f"Model loaded in {time.monotonic() - t0:.1f}s", file=sys.stderr)
    return pipeline


def classify_text(pipeline, text, labels, threshold=0.3):
    results = pipeline(text, labels, threshold=threshold)
    if results and len(results) > 0:
        return results[0]
    return []


def open_db():
    db_path = os.path.expanduser("~/.nmem/nmem.db")
    if not os.path.exists(db_path):
        print(f"DB not found: {db_path}", file=sys.stderr)
        sys.exit(1)
    conn = sqlite3.connect(db_path)
    return conn


def sample_prompts(conn, limit=30):
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


def eval_corpus(pipeline, corpus_path, threshold=0.3):
    """Evaluate against a labeled corpus and report accuracy."""
    with open(corpus_path) as f:
        corpus = json.load(f)

    type_correct = 0
    type_total = 0
    concept_tp = 0  # true positives
    concept_fp = 0  # false positives
    concept_fn = 0  # false negatives
    type_confusion = {}  # (expected, predicted) -> count
    per_type_correct = {}
    per_type_total = {}
    total_ms = 0

    results = []

    for entry in corpus:
        text = entry["text"][:500]
        expected_type = entry["type"]
        expected_concepts = set(entry.get("concepts", []))

        t0 = time.monotonic()
        type_results = classify_text(pipeline, text, TYPE_LABELS, threshold)
        concept_results = classify_text(pipeline, text, CONCEPT_LABELS, threshold)
        elapsed_ms = round((time.monotonic() - t0) * 1000)
        total_ms += elapsed_ms

        # Predicted type = highest score
        if type_results:
            best = max(type_results, key=lambda x: x["score"])
            predicted_type = best["label"]
            type_score = best["score"]
        else:
            predicted_type = "?"
            type_score = 0.0

        # Predicted concepts
        predicted_concepts = set()
        for r in concept_results:
            predicted_concepts.add(r["label"])

        # Type accuracy
        type_total += 1
        per_type_total[expected_type] = per_type_total.get(expected_type, 0) + 1
        match = predicted_type == expected_type
        if match:
            type_correct += 1
            per_type_correct[expected_type] = per_type_correct.get(expected_type, 0) + 1

        # Concept precision/recall
        tp = len(predicted_concepts & expected_concepts)
        fp = len(predicted_concepts - expected_concepts)
        fn = len(expected_concepts - predicted_concepts)
        concept_tp += tp
        concept_fp += fp
        concept_fn += fn

        # Track confusion
        key = (expected_type, predicted_type)
        type_confusion[key] = type_confusion.get(key, 0) + 1

        preview = text[:55].replace("\n", " ")
        mark = "+" if match else "X"
        print(f"  [{mark}] {preview:<55} exp={expected_type:<15} got={predicted_type:<15} {type_score:.2f}  {elapsed_ms}ms")

        if not match or (fp > 0 or fn > 0):
            if expected_concepts or predicted_concepts:
                exp_c = ",".join(sorted(expected_concepts)) if expected_concepts else "-"
                got_c = ",".join(sorted(predicted_concepts)) if predicted_concepts else "-"
                if exp_c != got_c:
                    print(f"       concepts: exp=[{exp_c}] got=[{got_c}]")

        results.append({
            "id": entry["id"],
            "source": entry["source"],
            "expected_type": expected_type,
            "predicted_type": predicted_type,
            "type_match": match,
            "expected_concepts": sorted(expected_concepts),
            "predicted_concepts": sorted(predicted_concepts),
            "concept_tp": tp, "concept_fp": fp, "concept_fn": fn,
            "latency_ms": elapsed_ms,
        })

    # Summary
    print(f"\n{'='*70}")
    print(f"TYPE ACCURACY: {type_correct}/{type_total} = {type_correct/type_total*100:.1f}%")
    print(f"Avg latency: {total_ms // type_total}ms")

    # Per-type breakdown
    print(f"\nPer-type accuracy:")
    for t in TYPE_LABELS:
        total = per_type_total.get(t, 0)
        correct = per_type_correct.get(t, 0)
        if total > 0:
            print(f"  {t:<16} {correct}/{total} = {correct/total*100:.0f}%")
        else:
            print(f"  {t:<16} (no examples)")

    # Concept metrics
    precision = concept_tp / (concept_tp + concept_fp) if (concept_tp + concept_fp) > 0 else 0
    recall = concept_tp / (concept_tp + concept_fn) if (concept_tp + concept_fn) > 0 else 0
    f1 = 2 * precision * recall / (precision + recall) if (precision + recall) > 0 else 0
    print(f"\nCONCEPT METRICS:")
    print(f"  Precision: {precision:.3f}  (tp={concept_tp}, fp={concept_fp})")
    print(f"  Recall:    {recall:.3f}  (tp={concept_tp}, fn={concept_fn})")
    print(f"  F1:        {f1:.3f}")

    # Confusion matrix (top errors)
    errors = {k: v for k, v in type_confusion.items() if k[0] != k[1]}
    if errors:
        print(f"\nTop type confusions:")
        for (exp, got), count in sorted(errors.items(), key=lambda x: -x[1])[:10]:
            print(f"  {exp} → {got}: {count}")

    out_path = "/tmp/nmem-zeroshot-corpus-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


def main():
    parser = argparse.ArgumentParser(description="Zero-shot prompt classification")
    parser.add_argument("--text", help="Classify a single text string")
    parser.add_argument("--corpus", help="Evaluate against a labeled corpus JSON file")
    parser.add_argument("--limit", type=int, default=30)
    parser.add_argument("--device", default="cuda:0", help="Device: cuda:0 or cpu")
    parser.add_argument("--threshold", type=float, default=0.3, help="Score threshold")
    args = parser.parse_args()

    pipeline = load_pipeline(args.device)

    if args.corpus:
        eval_corpus(pipeline, args.corpus, args.threshold)
        return

    if args.text:
        # Single text mode
        print("\n--- Types ---")
        type_results = classify_text(pipeline, args.text, TYPE_LABELS, args.threshold)
        for r in sorted(type_results, key=lambda x: x["score"], reverse=True):
            print(f"  {r['label']:<20} {r['score']:.3f}")

        print("\n--- Concepts ---")
        concept_results = classify_text(pipeline, args.text, CONCEPT_LABELS, args.threshold)
        for r in sorted(concept_results, key=lambda x: x["score"], reverse=True):
            print(f"  {r['label']:<20} {r['score']:.3f}")
        return

    # Batch mode from DB
    conn = open_db()
    rows = sample_prompts(conn, args.limit)
    conn.close()

    if not rows:
        print("No prompts found", file=sys.stderr)
        sys.exit(1)

    print(f"\nClassifying {len(rows)} prompts...")
    print()

    results = []
    total_ms = 0

    for pid, source, content, project in rows:
        text = content[:500]
        preview = content[:60].replace("\n", " ")

        t0 = time.monotonic()
        type_results = classify_text(pipeline, text, TYPE_LABELS, args.threshold)
        concept_results = classify_text(pipeline, text, CONCEPT_LABELS, args.threshold)
        elapsed_ms = round((time.monotonic() - t0) * 1000)
        total_ms += elapsed_ms

        # Best type
        if type_results:
            best_type = max(type_results, key=lambda x: x["score"])
            type_id = best_type["label"].split(":")[0]
            type_score = best_type["score"]
        else:
            type_id = "?"
            type_score = 0.0

        # Concepts above threshold
        concepts = []
        for r in sorted(concept_results, key=lambda x: x["score"], reverse=True):
            concepts.append((r["label"], r["score"]))

        concepts_str = ", ".join(f"{c}({s:.2f})" for c, s in concepts[:3]) if concepts else "-"

        print(f"  [{source:>5}] {preview:<60} → {type_id:<16} {type_score:.2f}  [{concepts_str}]  {elapsed_ms}ms")

        results.append({
            "id": pid,
            "source": source,
            "preview": preview,
            "type": type_id,
            "type_score": type_score,
            "concepts": [{"id": c, "score": s} for c, s in concepts],
            "latency_ms": elapsed_ms,
        })

    # Summary
    print(f"\n--- Summary ---")
    print(f"Total: {len(results)} prompts")
    print(f"Avg latency: {total_ms // len(results)}ms")

    types = {}
    for r in results:
        t = r["type"]
        types[t] = types.get(t, 0) + 1
    print(f"Type distribution: {json.dumps(types, indent=2)}")

    concept_freq = {}
    for r in results:
        for c in r["concepts"]:
            concept_freq[c["id"]] = concept_freq.get(c["id"], 0) + 1
    print(f"Concept frequency: {json.dumps(concept_freq, indent=2)}")

    out_path = "/tmp/nmem-zeroshot-eval.json"
    with open(out_path, "w") as f:
        json.dump(results, f, indent=2)
    print(f"\nFull results: {out_path}")


if __name__ == "__main__":
    main()
