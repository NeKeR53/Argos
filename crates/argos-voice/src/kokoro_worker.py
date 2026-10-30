"""Synthèse Kokoro locale : un seul processus pour toute la session, le modèle reste chargé.

Chaque ligne reçue sur stdin est une phrase, encodée en JSON. Pour chacune, la réponse sur
stdout est « ok », ou « erreur <message> » si la phrase n'a pas pu être dite. Le texte ne
quitte jamais la machine.

La sortie audio reste ouverte tout du long et joue du silence entre deux phrases : sans
cela, macOS met le haut-parleur en veille après quelques secondes muettes, et le réveil
avale le début du son suivant — la première phrase après une pause n'était pas entendue."""
import json
import sys
import threading

import numpy as np
import sounddevice as sd
from kokoro_onnx import SAMPLE_RATE, Kokoro

# Marge au-delà de la durée d'une phrase avant de tenir la sortie pour perdue.
PLAY_MARGIN_S = 2.0


class Output:
    """Flux de sortie permanent : il lit la phrase confiée, du silence sinon."""

    def __init__(self):
        self.lock = threading.Lock()
        self.samples = np.zeros(0, dtype=np.float32)
        self.drained = threading.Event()
        self.drained.set()
        self.stream = sd.OutputStream(
            samplerate=SAMPLE_RATE, channels=1, dtype="float32", callback=self.fill
        )
        self.stream.start()

    def fill(self, out, frames, _time, _status):
        with self.lock:
            count = min(frames, len(self.samples))
            out[:count, 0] = self.samples[:count]
            out[count:, 0] = 0
            self.samples = self.samples[count:]
            if not len(self.samples):
                self.drained.set()

    def play(self, samples):
        """Joue `samples` et rend la main quand tout est parti au haut-parleur."""
        with self.lock:
            self.samples = np.asarray(samples, dtype=np.float32).reshape(-1)
            self.drained.clear()
        if not self.drained.wait(len(self.samples) / SAMPLE_RATE + PLAY_MARGIN_S):
            raise RuntimeError("la sortie audio ne répond plus")

    def close(self):
        try:
            self.stream.close()
        except Exception:
            pass


model, voices, voice = sys.argv[1:4]
try:
    # La sortie d'abord : le haut-parleur se réveille pendant le chargement du modèle.
    output = Output()
except Exception:  # retentée à la première phrase, qui dira l'erreur
    output = None
try:
    kokoro = Kokoro(model, voices)
    failure = None
except Exception as error:  # le chargement échoue : chaque phrase le dira
    kokoro = None
    failure = str(error).replace("\n", " ")

for line in sys.stdin:
    try:
        if failure is not None:
            raise RuntimeError(failure)
        text = json.loads(line)
        if text.strip():
            samples, _rate = kokoro.create(text, voice=voice, lang="fr-fr")
            if output is None:
                output = Output()
            try:
                output.play(samples)
            except Exception:
                # Périphérique perdu (casque débranché…) : la phrase suivante en rouvre un.
                output.close()
                output = None
                raise
        print("ok", flush=True)
    except Exception as error:
        print("erreur " + str(error).replace("\n", " "), flush=True)
