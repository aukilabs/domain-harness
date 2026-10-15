#!/usr/bin/env python3
"""Say one greeting through BBOS's speaker: piper speaks the text into a WAV,
and play-bracketbot-speech.py plays it without taking over the audio device.

Input: one JSON line on stdin, {"text": "..."}. The text goes to piper on stdin,
never as a command or an option. piper and its voice live under ~/tools:
  ~/tools/piper/piper
  ~/tools/piper-voices/en_US-lessac-medium.onnx
"""
import json
from pathlib import Path
import subprocess
import sys
import tempfile

TOOLS = Path.home()/'tools'
PIPER = TOOLS/'piper'/'piper'
VOICE = TOOLS/'piper-voices'/'en_US-lessac-medium.onnx'
MAX_TEXT = 200


def speak(text, run=subprocess.run):
    text = ' '.join(str(text).split())[:MAX_TEXT]
    if not text:
        raise ValueError('nothing to say')
    here = Path(__file__).resolve().parent
    with tempfile.TemporaryDirectory(prefix='bracketbot-greet-') as directory:
        wave = Path(directory)/'greeting.wav'
        run([str(PIPER), '--model', str(VOICE), '--output_file', str(wave)], input=text.encode(),
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60, check=True)
        run([sys.executable, str(here/'bracketbot-python.py'), '--daemon', 'speaker',
             str(here/'play-bracketbot-speech.py'), str(wave)],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60, check=True)


def main():
    request = json.loads(sys.stdin.readline() or '{}')
    speak(request.get('text', ''))


if __name__ == '__main__':
    main()
