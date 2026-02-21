#!/usr/bin/env python3
"""
Train TF-IDF + LinearSVC for think/act classification.
Export model weights as JSON for Rust-native inference.

Usage:
    python3 tools/classify-train.py --corpus tools/corpus.json
    python3 tools/classify-train.py --corpus tools/corpus-think-act-7k.json --output models/think-act.json

Requires: scikit-learn, numpy
"""

import argparse
import json
import sys

import joblib
import numpy as np
from sklearn.feature_extraction.text import TfidfVectorizer
from sklearn.metrics import classification_report
from sklearn.model_selection import StratifiedKFold, cross_val_predict
from sklearn.pipeline import FeatureUnion, Pipeline
from sklearn.svm import LinearSVC


def load_corpus(path):
    with open(path) as f:
        corpus = json.load(f)

    texts = [e["text"] for e in corpus]
    labels = [e["type"] for e in corpus]

    think_count = labels.count("think")
    act_count = labels.count("act")
    print(f"Corpus: {len(corpus)} entries ({think_count} think, {act_count} act)")

    return texts, labels


def build_pipeline():
    """TF-IDF with char + word n-grams → LinearSVC."""
    features = FeatureUnion([
        ("word", TfidfVectorizer(
            analyzer="word",
            ngram_range=(1, 2),
            sublinear_tf=True,
            max_features=3000,
            min_df=2,
            max_df=0.95,
            strip_accents="unicode",
            binary=True,
        )),
        ("char", TfidfVectorizer(
            analyzer="char_wb",
            ngram_range=(3, 5),
            sublinear_tf=True,
            max_features=2000,
            min_df=2,
            max_df=0.95,
            strip_accents="unicode",
        )),
    ])

    pipeline = Pipeline([
        ("features", features),
        ("clf", LinearSVC(
            C=1.0,
            max_iter=2000,
            class_weight="balanced",
        )),
    ])

    return pipeline


def evaluate(pipeline, texts, labels):
    """Stratified k-fold cross-validation with per-class metrics."""
    cv = StratifiedKFold(n_splits=5, shuffle=True, random_state=42)
    y_pred = cross_val_predict(pipeline, texts, labels, cv=cv)
    print("\nCross-validation results (5-fold stratified):")
    print(classification_report(labels, y_pred, digits=3))
    return y_pred


def export_json(pipeline, output_path, labels):
    """Export trained model weights as JSON for Rust inference.

    The pipeline structure is:
      features (FeatureUnion) → clf (LinearSVC)

    The FeatureUnion contains word and char TfidfVectorizers.
    LinearSVC exposes coef_ and intercept_ directly.
    """
    features = pipeline.named_steps["features"]
    clf = pipeline.named_steps["clf"]

    word_vec = features.transformer_list[0][1]  # ("word", TfidfVectorizer)
    char_vec = features.transformer_list[1][1]  # ("char", TfidfVectorizer)

    coef = clf.coef_[0]
    intercept = float(clf.intercept_[0])

    # The FeatureUnion concatenates [word_features, char_features]
    word_n = len(word_vec.vocabulary_)
    char_n = len(char_vec.vocabulary_)

    word_weights = coef[:word_n].tolist()
    char_weights = coef[word_n:word_n + char_n].tolist()

    # Determine class mapping — sklearn sorts classes alphabetically
    # classes_ = ['act', 'think'] → positive class is index 1 (think)
    # coef_[0] gives weights for class 1 vs class 0
    # So positive score → think, negative score → act
    classes = clf.classes_.tolist()

    model = {
        "classes": classes,
        "word": {
            "vocabulary": {k: int(v) for k, v in word_vec.vocabulary_.items()},
            "idf": word_vec.idf_.tolist(),
            "weights": word_weights,
            "ngram_range": list(word_vec.ngram_range),
            "binary": bool(word_vec.binary),
            "sublinear_tf": bool(word_vec.sublinear_tf),
        },
        "char": {
            "vocabulary": {k: int(v) for k, v in char_vec.vocabulary_.items()},
            "idf": char_vec.idf_.tolist(),
            "weights": char_weights,
            "ngram_range": list(char_vec.ngram_range),
            "binary": False,
            "sublinear_tf": bool(char_vec.sublinear_tf),
        },
        "bias": intercept,
    }

    with open(output_path, "w") as f:
        json.dump(model, f)

    size_kb = len(json.dumps(model)) / 1024
    print(f"\nExported model: {output_path} ({size_kb:.0f} KB)")
    print(f"  Word features: {word_n}, Char features: {char_n}")
    print(f"  Classes: {classes} (positive = {classes[1]})")
    print(f"  Bias: {intercept:.4f}")


def main():
    parser = argparse.ArgumentParser(description="Train think/act classifier")
    parser.add_argument("--corpus", required=True, help="Labeled corpus JSON")
    parser.add_argument("--output", default="models/think-act.json", help="Model output path")
    parser.add_argument("--pickle", help="Also save sklearn pipeline as pickle")
    parser.add_argument("--no-eval", action="store_true", help="Skip cross-validation")
    args = parser.parse_args()

    texts, labels = load_corpus(args.corpus)

    if len(texts) < 20:
        print("Corpus too small (need at least 20 entries)", file=sys.stderr)
        sys.exit(1)

    pipeline = build_pipeline()

    if not args.no_eval:
        evaluate(pipeline, texts, labels)

    # Train on full corpus
    print("Training on full corpus...")
    pipeline.fit(texts, labels)

    # Ensure output directory exists
    import os
    os.makedirs(os.path.dirname(args.output) or ".", exist_ok=True)

    export_json(pipeline, args.output, labels)

    if args.pickle:
        joblib.dump(pipeline, args.pickle)
        print(f"Pickle saved: {args.pickle}")


if __name__ == "__main__":
    main()
