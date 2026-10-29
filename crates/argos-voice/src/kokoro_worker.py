"""Local Kokoro synthesis; text is read from stdin, never sent over the network."""
import sys
from kokoro_onnx import Kokoro
import sounddevice as sd

model, voices, voice = sys.argv[1:4]
text = sys.stdin.read()
if text.strip():
    kokoro = Kokoro(model, voices)
    samples, rate = kokoro.create(text, voice=voice, lang="fr-fr")
    sd.play(samples, rate)
    sd.wait()
