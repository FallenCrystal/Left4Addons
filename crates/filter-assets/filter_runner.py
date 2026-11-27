#!/usr/bin/env python3
"""
Managed Left4Filters HTTP runner for Left4Addons (Desktop Client & Catalog Server).
Wraps an unmodified Left4Filters checkout (`deploy/l4filter.py`) without altering
any tracked files in the repository, supporting:
  - Dynamic loopback port binding (`--port 0`) with structured ready banner
  - Parent-process death detection (`--watch-stdin`) so runners never stay orphaned
  - Decompression-bomb, concurrency, chunked-encoding, and payload bounds hardening
  - Single-vision-pass multimodal + unimodal (text / title / description / per-image) scoring
  - Live runtime model parameter updates (`POST /configure`) protected by constant-time bearer token
"""
from __future__ import annotations

import argparse
import base64
import hmac
import io
import json
import math
import os
import re
import sys
import threading
from collections import OrderedDict
from hashlib import sha256
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Sequence

import numpy as np
from PIL import Image

MAX_IMAGE_PIXELS = 16_777_216  # 4096 x 4096 cap against decompression bombs
MAX_IMAGE_DIMENSION = 4096
MAX_SDK_RESIZE_ASPECT = 8
MAX_SINGLE_IMAGE_BYTES = 16 * 1024 * 1024
MAX_REQUEST_BODY_BYTES = 64 * 1024 * 1024
MAX_CHUNK_LINE_BYTES = 65536
MAX_TRAILER_BYTES = 65536
MAX_TITLE_CHARS = 4096
MAX_DESC_CHARS = 65536
MAX_TAGS_COUNT = 64
MAX_TAG_CHARS = 256
VISION_CACHE_SIZE = 256
_CHUNK_SIZE_RE = re.compile(rb"^[0-9a-fA-F]{1,16}$")
_CONTENT_LENGTH_RE = re.compile(r"^[0-9]{1,16}$")

Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS


def resolve_deploy_dir(raw_dir: str) -> Path:
    base = Path(raw_dir).resolve()
    if (base / "l4filter.py").is_file():
        return base
    if (base / "deploy" / "l4filter.py").is_file():
        return base / "deploy"
    raise FileNotFoundError(f"l4filter.py not found under {base} or {base / 'deploy'}")


def finite_float(v: Any, default: float) -> float:
    try:
        f = float(v)
    except (TypeError, ValueError):
        return default
    return f if math.isfinite(f) else default


def clamp_prob(p: float) -> float:
    f = finite_float(p, 0.0)
    return max(1e-6, min(1.0 - 1e-6, f))


def round_prob(p: float) -> float:
    f = finite_float(p, 0.0)
    return round(max(0.0, min(1.0, f)), 6)


def to_logit(p: float) -> float:
    c = clamp_prob(p)
    return math.log(c / (1.0 - c))


def from_logit(x: float) -> float:
    if not math.isfinite(x):
        return 0.0
    if x >= 40.0:
        return 1.0
    if x <= -40.0:
        return 0.0
    return 1.0 / (1.0 + math.exp(-x))


def adjust_triplet(
    ov: float,
    gore: float,
    nsfw: float,
    gore_weight: float,
    nsfw_weight: float,
    temperature: float,
    bias: float,
) -> tuple[float, float, float]:
    gw = max(0.0, min(3.0, finite_float(gore_weight, 1.0)))
    nw = max(0.0, min(3.0, finite_float(nsfw_weight, 1.0)))
    temp = max(0.25, min(4.0, finite_float(temperature, 1.0)))
    b = max(-5.0, min(5.0, finite_float(bias, 0.0)))

    if (
        abs(gw - 1.0) < 1e-5
        and abs(nw - 1.0) < 1e-5
        and abs(temp - 1.0) < 1e-5
        and abs(b) < 1e-5
    ):
        return round_prob(ov), round_prob(gore), round_prob(nsfw)

    def adj_single(p: float, w: float) -> float:
        if w <= 1e-6:
            return 0.0
        return from_logit((to_logit(p) + math.log(w)) / temp + b)

    g_adj = adj_single(gore, gw)
    n_adj = adj_single(nsfw, nw)

    if gw <= 1e-6 and nw <= 1e-6:
        ov_adj = 0.0
    elif gw <= 1e-6:
        ov_adj = n_adj
    elif nw <= 1e-6:
        ov_adj = g_adj
    elif abs(gw - 1.0) < 1e-5 and abs(nw - 1.0) < 1e-5:
        ov_adj = from_logit(to_logit(ov) / temp + b)
    else:
        g_weighted = from_logit(to_logit(gore) + math.log(gw))
        n_weighted = from_logit(to_logit(nsfw) + math.log(nw))
        orig_or = clamp_prob(1.0 - (1.0 - clamp_prob(gore)) * (1.0 - clamp_prob(nsfw)))
        new_or = clamp_prob(1.0 - (1.0 - g_weighted) * (1.0 - n_weighted))
        delta_scale = min(gw, nw) / max(gw, nw, 1.0)
        delta_logit = (to_logit(ov) - to_logit(orig_or)) * delta_scale
        ov_adj = from_logit((to_logit(new_or) + delta_logit) / temp + b)

    return round_prob(ov_adj), round_prob(g_adj), round_prob(n_adj)


def aggregate_rows(rows: np.ndarray, mode: str) -> np.ndarray:
    """Aggregate (N, 3) probability matrix across gallery images into (3,)."""
    if rows.shape[0] == 1:
        return rows[0]
    if mode == "mean":
        return rows.mean(axis=0)
    if mode == "top2_mean":
        if rows.shape[0] <= 2:
            return rows.mean(axis=0)
        part = np.sort(rows, axis=0)[-2:, :]
        return part.mean(axis=0)
    return rows.max(axis=0)


def decode_b64_image(raw_item: Any) -> bytes:
    if not isinstance(raw_item, str):
        raise ValueError("image_base64 must be a string")
    s = raw_item.strip()
    if s.startswith("data:"):
        comma = s.find(",")
        if comma == -1:
            raise ValueError("invalid data URL")
        s = s[comma + 1 :].strip()
    if len(s) > (MAX_SINGLE_IMAGE_BYTES * 4) // 3 + 64:
        raise ValueError("image_too_large")
    decoded = base64.b64decode(s, validate=False)
    if len(decoded) > MAX_SINGLE_IMAGE_BYTES:
        raise ValueError("image_too_large")
    return decoded


def check_image_dimensions(image: Image.Image) -> None:
    w, h = image.size
    if not (0 < w <= MAX_IMAGE_DIMENSION and 0 < h <= MAX_IMAGE_DIMENSION
            and w * h <= MAX_IMAGE_PIXELS):
        raise ValueError("image_dimensions_out_of_bounds")


def preprocess_bounded(image: Image.Image, size: int) -> np.ndarray:
    # Sample the same center crop as the SDK without allocating the enlarged long edge.
    w, h = image.size
    scale = size / min(w, h)
    rw, rh = max(size, round(w * scale)), max(size, round(h * scale))
    x, y = (rw - size) // 2, (rh - size) // 2
    box = (x * w / rw, y * h / rh, (x + size) * w / rw, (y + size) * h / rh)
    resized = image.resize((size, size), Image.Resampling.BILINEAR, box=box)
    return np.asarray(resized, dtype=np.float32).transpose(2, 0, 1) / 255.0


def bounded_sdk_input(raw: bytes) -> bytes | Image.Image:
    with Image.open(io.BytesIO(raw)) as im:
        check_image_dimensions(im)
        w, h = im.size
        edge = min(w, h)
        if max(w, h) <= MAX_SDK_RESIZE_ASPECT * edge:
            return raw
        x, y = (w - edge) // 2, (h - edge) // 2
        return im.crop((x, y, x + edge, y + edge)).convert("RGB")


class RunnerEngine:
    def __init__(self, deploy_dir: Path, initial_params: dict[str, Any], max_concurrency: int = 4) -> None:
        self.deploy_dir = deploy_dir
        sys.path.insert(0, str(deploy_dir))
        import l4filter as lf  # type: ignore

        Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS
        self._lf = lf
        self._lock = threading.RLock()
        self._sem = threading.BoundedSemaphore(max(1, min(16, int(max_concurrency))))
        # The managed runner uses bounded preprocessing and caches its feature batches.
        self._vision_cache: OrderedDict[tuple[bytes, ...], tuple[np.ndarray, np.ndarray]] = OrderedDict()
        if (deploy_dir / "corpse_head.onnx").is_file() and not (deploy_dir / "model_config.json").is_file():
            raise FileNotFoundError("model_config.json is required for corpse_head.onnx")
        self.params = self._normalize_params(initial_params)
        self.clf = lf.Left4Filter(str(deploy_dir), num_threads=self.params["threads"])

    @staticmethod
    def _normalize_params(raw: dict[str, Any]) -> dict[str, Any]:
        if not isinstance(raw, dict):
            raw = {}

        def pick(camel: str, snake: str, default: Any) -> Any:
            val = raw.get(camel)
            if val is None:
                val = raw.get(snake)
            return default if val is None else val

        agg = str(pick("galleryAggregation", "gallery_aggregation", "max") or "max").lower()
        if agg not in ("max", "top2_mean", "mean"):
            agg = "max"
        return {
            "threads": max(1, min(16, int(finite_float(raw.get("threads", 4), 4.0)))),
            "enable_text": bool(pick("enableText", "enable_text", True)),
            "enable_image": bool(pick("enableImage", "enable_image", True)),
            "max_images": max(1, min(8, int(finite_float(pick("maxImages", "max_images", 2), 2.0)))),
            "gallery_aggregation": agg,
            "gore_weight": max(0.0, min(3.0, finite_float(pick("goreWeight", "gore_weight", 1.0), 1.0))),
            "nsfw_weight": max(0.0, min(3.0, finite_float(pick("nsfwWeight", "nsfw_weight", 1.0), 1.0))),
            "temperature": max(0.25, min(4.0, finite_float(raw.get("temperature", 1.0), 1.0))),
            "bias": max(-5.0, min(5.0, finite_float(raw.get("bias", 0.0), 0.0))),
        }

    def image_limit(self) -> int:
        with self._lock:
            return int(self.params["max_images"]) if self.params["enable_image"] else 0

    def update_params(self, next_raw: dict[str, Any]) -> dict[str, Any]:
        normalized = self._normalize_params(next_raw)
        with self._lock:
            old_threads = self.params["threads"]
            if normalized["threads"] != old_threads:
                new_clf = self._lf.Left4Filter(str(self.deploy_dir), num_threads=normalized["threads"])
                Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS
                self.clf = new_clf
                self._vision_cache.clear()
            self.params = normalized
            return dict(self.params)

    def info(self) -> dict[str, Any]:
        with self._lock:
            cfg = getattr(self.clf, "config", {}) or {}
            provider = getattr(self.clf, "vision_provider", None)
            return {
                "status": "ok",
                "model": cfg.get("model_name", "Left4FiltersModel"),
                "version": cfg.get("version", "1.0.0"),
                "vision_provider": provider if isinstance(provider, str) else None,
                "params": dict(self.params),
            }

    def _adjust(self, trip: Sequence[float] | np.ndarray, p: dict[str, Any]) -> tuple[float, float, float]:
        return adjust_triplet(
            float(trip[0]),
            float(trip[1]),
            float(trip[2]),
            p["gore_weight"],
            p["nsfw_weight"],
            p["temperature"],
            p["bias"],
        )

    def predict(
        self,
        title: str,
        description: str,
        tags: list[str] | None,
        raw_images: list[bytes],
    ) -> dict[str, Any]:
        with self._sem:
            with self._lock:
                p = dict(self.params)
                clf = self.clf
                lf = self._lf

            use_title = title[:MAX_TITLE_CHARS] if p["enable_text"] else ""
            use_desc = description[:MAX_DESC_CHARS] if p["enable_text"] else ""
            use_tags = tags[:MAX_TAGS_COUNT] if (p["enable_text"] and tags) else None
            has_txt = bool(use_title.strip() or use_desc.strip() or (use_tags and len(use_tags) > 0))
            imgs = raw_images[: p["max_images"]] if p["enable_image"] else []

            if not has_txt and not imgs:
                return {
                    "prob": 0.0,
                    "goreProb": 0.0,
                    "nsfwProb": 0.0,
                    "textProb": None,
                    "textGoreProb": None,
                    "textNsfwProb": None,
                    "titleProb": None,
                    "descriptionProb": None,
                    "imageProb": None,
                    "imageGoreProb": None,
                    "imageNsfwProb": None,
                    "perImageProbs": [],
                }

            fast_attrs = all(
                hasattr(clf, attr) for attr in ("vis_sess", "head_sess", "vocab_data", "txt_dim", "img_size")
            ) and all(
                hasattr(lf, fn)
                for fn in ("extract_text_features", "extract_visual_priors")
            )

            if fast_attrs:
                return self._predict_fast(clf, lf, p, use_title, use_desc, use_tags, has_txt, imgs)
            return self._predict_sdk_fallback(clf, p, use_title, use_desc, use_tags, has_txt, imgs)

    def _run_text_only(self, clf: Any, lf: Any, title: str, desc: str, tags: list[str] | None) -> np.ndarray:
        vis_prior_dim = int(getattr(lf, "VISUAL_PRIOR_DIM", 24))
        th, tl = lf.extract_text_features(title, desc, tags, clf.vocab_data)
        return clf.head_sess.run(
            ["probs"],
            {
                "img_emb": np.zeros((1, 512), dtype=np.float32),
                "img_priors": np.zeros((1, vis_prior_dim), dtype=np.float32),
                "txt_hash": th[None, ...],
                "txt_lex": tl[None, ...],
                "has_img": np.zeros((1, 1), dtype=np.float32),
                "has_txt": np.ones((1, 1), dtype=np.float32),
            },
        )[0][0]

    def _image_features(self, clf: Any, lf: Any, imgs: list[bytes]) -> tuple[np.ndarray, np.ndarray]:
        # Keep the batch order because the vision encoder receives the entire batch.
        key = tuple(sha256(raw).digest() for raw in imgs)
        with self._lock:
            if clf is self.clf and key in self._vision_cache:
                self._vision_cache.move_to_end(key)
                return self._vision_cache[key]

        Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS
        pixels, priors = [], []
        for raw in imgs:
            with Image.open(io.BytesIO(raw)) as source:
                check_image_dimensions(source)
                with source.convert("RGB") as image:
                    pixels.append(preprocess_bounded(image, clf.img_size))
                    priors.append(lf.extract_visual_priors(image))
        embeddings = clf.vis_sess.run(["img_emb"], {"pixel_values": np.stack(pixels)})[0]
        features = (embeddings, np.stack(priors))
        with self._lock:
            if clf is self.clf:
                self._vision_cache[key] = features
                self._vision_cache.move_to_end(key)
                if len(self._vision_cache) > VISION_CACHE_SIZE:
                    self._vision_cache.popitem(last=False)
        return features

    def _corpse_fields(self, clf: Any, p: dict[str, Any], embeddings: np.ndarray | None) -> dict[str, Any]:
        session = getattr(clf, "corpse_sess", None)
        if session is None:
            return {}
        if embeddings is None:
            return {"corpseProb": 0.0, "corpsePileProb": 0.0}

        scores = session.run(["corpse_probs"], {"img_emb": embeddings})[0]
        corpse, pile = scores.max(axis=0)
        result: dict[str, Any] = {"corpseProb": round_prob(corpse), "corpsePileProb": round_prob(pile)}
        if p["gore_weight"] > 1e-6:
            hits = np.flatnonzero(np.any(scores >= clf.corpse_thresholds, axis=1)).tolist()
            if hits:
                result["corpseImageIndices"] = hits
                result["corpseReviewFloor"] = round_prob(clf.review_threshold)
        return result

    def _predict_fast(
        self,
        clf: Any,
        lf: Any,
        p: dict[str, Any],
        title: str,
        desc: str,
        tags: list[str] | None,
        has_txt: bool,
        imgs: list[bytes],
    ) -> dict[str, Any]:
        lex_dim = int(getattr(lf, "LEXICAL_PRIOR_DIM", 18))
        text_trip: tuple[float, float, float] | None = None
        title_prob: float | None = None
        desc_prob: float | None = None

        if has_txt:
            th, tl = lf.extract_text_features(title, desc, tags, clf.vocab_data)
            vis_prior_dim = int(getattr(lf, "VISUAL_PRIOR_DIM", 24))
            raw_txt = clf.head_sess.run(
                ["probs"],
                {
                    "img_emb": np.zeros((1, 512), dtype=np.float32),
                    "img_priors": np.zeros((1, vis_prior_dim), dtype=np.float32),
                    "txt_hash": th[None, ...],
                    "txt_lex": tl[None, ...],
                    "has_img": np.zeros((1, 1), dtype=np.float32),
                    "has_txt": np.ones((1, 1), dtype=np.float32),
                },
            )[0][0]
            text_trip = self._adjust(raw_txt, p)

            if title.strip() or (tags and len(tags) > 0):
                raw_t = self._run_text_only(clf, lf, title, "", tags)
                title_prob = self._adjust(raw_t, p)[0]
            if desc.strip():
                raw_d = self._run_text_only(clf, lf, "", desc, None)
                desc_prob = self._adjust(raw_d, p)[0]
        else:
            th = np.zeros(clf.txt_dim, dtype=np.float32)
            tl = np.zeros(lex_dim, dtype=np.float32)

        if not imgs:
            assert text_trip is not None
            return {
                **self._corpse_fields(clf, p, None),
                "prob": text_trip[0],
                "goreProb": text_trip[1],
                "nsfwProb": text_trip[2],
                "textProb": text_trip[0],
                "textGoreProb": text_trip[1],
                "textNsfwProb": text_trip[2],
                "titleProb": title_prob,
                "descriptionProb": desc_prob,
                "imageProb": None,
                "imageGoreProb": None,
                "imageNsfwProb": None,
                "perImageProbs": [],
            }

        emb_batch, pri_batch = self._image_features(clf, lf, imgs)
        corpse_fields = self._corpse_fields(clf, p, emb_batch)

        n = len(imgs)
        zero_th = np.zeros((n, clf.txt_dim), dtype=np.float32)
        zero_tl = np.zeros((n, lex_dim), dtype=np.float32)
        ones_n = np.ones((n, 1), dtype=np.float32)
        zeros_n = np.zeros((n, 1), dtype=np.float32)

        img_only_rows = clf.head_sess.run(
            ["probs"],
            {
                "img_emb": emb_batch,
                "img_priors": pri_batch,
                "txt_hash": zero_th,
                "txt_lex": zero_tl,
                "has_img": ones_n,
                "has_txt": zeros_n,
            },
        )[0]
        per_image_probs = [self._adjust(row, p)[0] for row in img_only_rows]
        img_agg_raw = aggregate_rows(img_only_rows, p["gallery_aggregation"])
        img_trip = self._adjust(img_agg_raw, p)

        if has_txt:
            th_batch = np.repeat(th[None, ...], n, axis=0)
            tl_batch = np.repeat(tl[None, ...], n, axis=0)
            joint_rows = clf.head_sess.run(
                ["probs"],
                {
                    "img_emb": emb_batch,
                    "img_priors": pri_batch,
                    "txt_hash": th_batch,
                    "txt_lex": tl_batch,
                    "has_img": ones_n,
                    "has_txt": ones_n,
                },
            )[0]
            joint_agg_raw = aggregate_rows(joint_rows, p["gallery_aggregation"])
            joint_trip = self._adjust(joint_agg_raw, p)
        else:
            joint_rows = img_only_rows
            joint_trip = img_trip

        fusion_prob = joint_trip[0]
        review_floor = corpse_fields.get("corpseReviewFloor")
        return {
            **corpse_fields,
            "prob": max(fusion_prob, review_floor or 0.0),
            "fusionProb": fusion_prob if review_floor is not None and fusion_prob <= review_floor else None,
            "goreProb": joint_trip[1],
            "nsfwProb": joint_trip[2],
            "textProb": text_trip[0] if text_trip else None,
            "textGoreProb": text_trip[1] if text_trip else None,
            "textNsfwProb": text_trip[2] if text_trip else None,
            "titleProb": title_prob,
            "descriptionProb": desc_prob,
            "imageProb": img_trip[0],
            "imageGoreProb": img_trip[1],
            "imageNsfwProb": img_trip[2],
            "perImageProbs": per_image_probs,
            "imageRows": img_only_rows.tolist(),
            "jointRows": joint_rows.tolist(),
        }

    def _predict_sdk_fallback(
        self,
        clf: Any,
        p: dict[str, Any],
        title: str,
        desc: str,
        tags: list[str] | None,
        has_txt: bool,
        imgs: list[bytes],
    ) -> dict[str, Any]:
        Image.MAX_IMAGE_PIXELS = MAX_IMAGE_PIXELS
        text_trip = None
        title_prob = None
        desc_prob = None
        if has_txt:
            rt = clf.predict(title=title, description=desc, tags=tags)
            text_trip = self._adjust([rt["prob"], rt["gore_prob"], rt["nsfw_prob"]], p)
            if title.strip() or (tags and len(tags) > 0):
                rt_t = clf.predict(title=title, description="", tags=tags)
                title_prob = self._adjust([rt_t["prob"], rt_t["gore_prob"], rt_t["nsfw_prob"]], p)[0]
            if desc.strip():
                rt_d = clf.predict(title="", description=desc, tags=None)
                desc_prob = self._adjust([rt_d["prob"], rt_d["gore_prob"], rt_d["nsfw_prob"]], p)[0]

        per_image_probs: list[float] = []
        img_trip = None
        rows, joint_rows = [], []
        for raw in imgs:
            im = bounded_sdk_input(raw)
            ri = clf.predict(image=im)
            row = [ri["prob"], ri["gore_prob"], ri["nsfw_prob"]]
            rows.append(row)
            per_image_probs.append(self._adjust(row, p)[0])
            if has_txt:
                rj = clf.predict(title=title, description=desc, tags=tags, image=im)
                joint_rows.append([rj["prob"], rj["gore_prob"], rj["nsfw_prob"]])
        if rows:
            agg = aggregate_rows(np.asarray(rows, dtype=np.float32), p["gallery_aggregation"])
            img_trip = self._adjust(agg, p)

        if joint_rows:
            joint_trip = self._adjust(
                aggregate_rows(np.asarray(joint_rows, dtype=np.float32), p["gallery_aggregation"]),
                p,
            )
        else:
            joint_rows = rows
            joint_trip = img_trip if img_trip is not None else text_trip
            assert joint_trip is not None

        return {
            "prob": joint_trip[0],
            "goreProb": joint_trip[1],
            "nsfwProb": joint_trip[2],
            "textProb": text_trip[0] if text_trip else None,
            "textGoreProb": text_trip[1] if text_trip else None,
            "textNsfwProb": text_trip[2] if text_trip else None,
            "titleProb": title_prob,
            "descriptionProb": desc_prob,
            "imageProb": img_trip[0] if img_trip else None,
            "imageGoreProb": img_trip[1] if img_trip else None,
            "imageNsfwProb": img_trip[2] if img_trip else None,
            "perImageProbs": per_image_probs,
            "imageRows": rows,
            "jointRows": joint_rows,
        }


def start_stdin_watchdog() -> None:
    def _watch() -> None:
        try:
            while True:
                chunk = sys.stdin.buffer.read(64)
                if not chunk:
                    break
        except Exception:
            pass
        os._exit(0)

    t = threading.Thread(target=_watch, name="l4a-stdin-watchdog", daemon=True)
    t.start()


def read_chunked_body(rfile: Any, max_bytes: int) -> bytes:
    out = bytearray()
    while True:
        line = rfile.readline(MAX_CHUNK_LINE_BYTES + 1)
        if not line:
            raise ValueError("incomplete_chunked_body")
        if len(line) > MAX_CHUNK_LINE_BYTES or not line.endswith((b"\r\n", b"\n")):
            raise ValueError("invalid_chunk_header_line")
        semi = line.find(b";")
        hex_part = (line[:semi] if semi != -1 else line).strip()
        if not _CHUNK_SIZE_RE.fullmatch(hex_part):
            raise ValueError("invalid_chunk_size")
        size = int(hex_part, 16)
        if size == 0:
            trailer_bytes = 0
            while True:
                trailer = rfile.readline(MAX_CHUNK_LINE_BYTES + 1)
                if not trailer:
                    raise ValueError("incomplete_chunked_trailer")
                if len(trailer) > MAX_CHUNK_LINE_BYTES or not trailer.endswith((b"\r\n", b"\n")):
                    raise ValueError("invalid_chunk_trailer_line")
                trailer_bytes += len(trailer)
                if trailer_bytes > MAX_TRAILER_BYTES:
                    raise OverflowError("chunk_trailers_too_large")
                if trailer in (b"\r\n", b"\n"):
                    break
            break
        if len(out) + size > max_bytes:
            raise OverflowError("payload_too_large")
        chunk = rfile.read(size)
        if len(chunk) != size:
            raise ValueError("truncated_chunk")
        out.extend(chunk)
        crlf = rfile.read(2)
        if crlf != b"\r\n":
            raise ValueError("invalid_chunk_terminator")
    return bytes(out)


def main() -> None:
    ap = argparse.ArgumentParser(description="Left4Addons managed Left4Filters HTTP runner")
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--port", type=int, default=0)
    ap.add_argument("--deploy-dir", required=True)
    ap.add_argument("--params-json", default="{}")
    ap.add_argument("--max-concurrency", type=int, default=4)
    ap.add_argument("--watch-stdin", action="store_true")
    args = ap.parse_args()

    if args.watch_stdin:
        start_stdin_watchdog()

    admin_token = os.environ.get("L4A_RUNNER_TOKEN", "").strip()
    expected_auth = f"Bearer {admin_token}".encode("utf-8") if admin_token else None
    deploy_dir = resolve_deploy_dir(args.deploy_dir)
    initial_params = json.loads(args.params_json) if args.params_json else {}
    engine = RunnerEngine(deploy_dir, initial_params, max_concurrency=args.max_concurrency)

    class Handler(BaseHTTPRequestHandler):
        timeout = 15

        def _send(self, code: int, payload: dict[str, Any]) -> None:
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def _check_token(self) -> bool:
            if expected_auth is None:
                return False
            auth = self.headers.get("Authorization", "").encode("utf-8")
            return hmac.compare_digest(auth, expected_auth)

        def _read_json(self) -> dict[str, Any]:
            te_headers = [v.strip().lower() for v in (self.headers.get_all("Transfer-Encoding") or [])]
            cl_headers = [v.strip() for v in (self.headers.get_all("Content-Length") or [])]
            if len(te_headers) > 1:
                raise ValueError("duplicate_transfer_encoding")
            if len(cl_headers) > 1:
                raise ValueError("duplicate_content_length")
            if te_headers and cl_headers:
                raise ValueError("conflicting_transfer_encoding_and_content_length")

            if te_headers:
                te = te_headers[0]
                if te != "chunked":
                    raise ValueError(f"unsupported_transfer_encoding:{te}")
                raw = read_chunked_body(self.rfile, MAX_REQUEST_BODY_BYTES)
            else:
                if not cl_headers:
                    raise ValueError("missing_content_length")
                cl_header = cl_headers[0]
                if not _CONTENT_LENGTH_RE.fullmatch(cl_header):
                    raise ValueError("invalid_content_length")
                length = int(cl_header)
                if length == 0:
                    return {}
                if length > MAX_REQUEST_BODY_BYTES:
                    raise OverflowError("payload_too_large")
                raw = self.rfile.read(length)
                if len(raw) != length:
                    raise ValueError("truncated_body")
            parsed = json.loads(raw.decode("utf-8"))
            if not isinstance(parsed, dict):
                raise ValueError("json_body_must_be_object")
            return parsed

        def do_GET(self) -> None:
            if self.path == "/health":
                self._send(200, engine.info())
            else:
                self._send(404, {"error": "not_found"})

        def do_POST(self) -> None:
            if self.path == "/configure":
                if not self._check_token():
                    self._send(401, {"error": "unauthorized"})
                    return
                try:
                    req = self._read_json()
                    updated = engine.update_params(req)
                    self._send(200, {"status": "ok", "params": updated})
                except OverflowError as exc:
                    self._send(413, {"error": str(exc)})
                except Exception as exc:
                    self._send(400, {"error": str(exc)})
                return

            if self.path != "/predict":
                self._send(404, {"error": "not_found"})
                return

            try:
                req = self._read_json()
                img_limit = engine.image_limit()
                imgs: list[bytes] = []
                if img_limit > 0:
                    b64_single = req.get("imageBase64") or req.get("image_base64")
                    if b64_single is not None and b64_single != "":
                        imgs.append(decode_b64_image(b64_single))
                    b64_many = req.get("imagesBase64")
                    if b64_many is None:
                        b64_many = req.get("images_base64")
                    if b64_many is not None:
                        if not isinstance(b64_many, list):
                            raise ValueError("images_base64 must be a JSON array")
                        for item in b64_many:
                            if len(imgs) >= img_limit:
                                break
                            if item is not None and item != "":
                                imgs.append(decode_b64_image(item))

                tags_raw = req.get("tags")
                tags: list[str] | None = None
                if tags_raw is not None:
                    if not isinstance(tags_raw, list):
                        raise ValueError("tags must be a JSON array")
                    tags = [str(x)[:MAX_TAG_CHARS] for x in tags_raw[:MAX_TAGS_COUNT]]

                title_val = req.get("title")
                desc_val = req.get("description")
                res = engine.predict(
                    title=str(title_val)[:MAX_TITLE_CHARS] if title_val is not None else "",
                    description=str(desc_val)[:MAX_DESC_CHARS] if desc_val is not None else "",
                    tags=tags,
                    raw_images=imgs,
                )
                self._send(200, res)
            except OverflowError as exc:
                self._send(413, {"error": str(exc)})
            except Exception as exc:
                self._send(400, {"error": str(exc)})

        def log_message(self, fmt: str, *args: Any) -> None:
            pass

    srv = ThreadingHTTPServer((args.host, args.port), Handler)
    srv.daemon_threads = True
    bound_host, bound_port = srv.server_address[0], srv.server_address[1]
    ready_payload = engine.info()
    ready_payload["host"] = bound_host
    ready_payload["port"] = bound_port
    print(f"L4A_FILTER_READY {json.dumps(ready_payload, ensure_ascii=False)}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
