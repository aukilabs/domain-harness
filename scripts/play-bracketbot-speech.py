#!/usr/bin/env python3
"""Play synthesized PCM WAV through BBOS, without taking over its audio device."""
import sys
import wave
import numpy as np
import soxr
from bbos import Config, Type, Writer


def main():
    config = Config('speaker')
    with wave.open(sys.argv[1], 'rb') as source:
        if source.getsampwidth() != 2 or source.getnchannels() != 1:
            raise ValueError('Expected mono 16-bit PCM speech')
        audio = np.frombuffer(source.readframes(source.getnframes()), dtype='<i2').astype(np.float32)/32768
        audio = soxr.resample(audio, source.getframerate(), config.sample_rate)
    audio = np.clip(audio*.6, -1, 1)
    audio = (audio*32767).astype(np.int16)
    audio = np.pad(audio, (0, (-len(audio)) % config.chunk_size))
    with Writer('speaker.audio', Type('speaker_audio')) as writer:
        for chunk in audio.reshape(-1, config.chunk_size):
            with writer.buf() as buffer:
                buffer['audio'] = np.repeat(chunk[:,None], config.channels, axis=1)


if __name__ == '__main__': main()
