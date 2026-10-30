"""Synthèse Kokoro locale : un seul processus pour toute la session, le modèle reste chargé.

Chaque ligne reçue sur stdin est une phrase, encodée en JSON. Pour chacune, la réponse sur
stdout est « ok », ou « erreur <message> » si la phrase n'a pas pu être dite. Le texte ne
quitte jamais la machine."""
import json
import sys

from kokoro_onnx import Kokoro
import sounddevice as sd

model, voices, voice = sys.argv[1:4]
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
            samples, rate = kokoro.create(text, voice=voice, lang="fr-fr")
            sd.play(samples, rate)
            sd.wait()
        print("ok", flush=True)
    except Exception as error:
        print("erreur " + str(error).replace("\n", " "), flush=True)
