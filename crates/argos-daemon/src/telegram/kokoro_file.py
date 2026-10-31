"""Synthèse Kokoro locale vers un fichier WAV, pour un vocal Telegram.

Arguments : modèle, voix, nom de la voix, fichier de sortie. Le texte arrive sur stdin.
Le WAV est écrit avec le module standard `wave`, sans dépendance de plus que Kokoro."""
import sys
import wave

import numpy as np
from kokoro_onnx import SAMPLE_RATE, Kokoro

model, voices, voice, output = sys.argv[1:5]
text = sys.stdin.read().strip()
samples, rate = Kokoro(model, voices).create(text or "…", voice=voice, lang="fr-fr")
pcm = (np.clip(np.asarray(samples, dtype=np.float32).reshape(-1), -1, 1) * 32767).astype("<i2")
with wave.open(output, "wb") as file:
    file.setnchannels(1)
    file.setsampwidth(2)
    file.setframerate(rate or SAMPLE_RATE)
    file.writeframes(pcm.tobytes())
