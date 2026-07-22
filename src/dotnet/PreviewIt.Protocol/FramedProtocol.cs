using System;
using System.IO;

namespace PreviewIt.Protocol
{
    public static class FramedProtocol
    {
        public const int MaxControlFrame = 1024 * 1024;

        public static byte[] Encode(byte[] payload)
        {
            if (payload == null)
            {
                throw new ArgumentNullException(nameof(payload));
            }

            if (payload.Length > MaxControlFrame)
            {
                throw new InvalidDataException(
                    $"Control frame payload is too large: {payload.Length} bytes.");
            }

            var length = (uint)payload.Length;
            var frame = new byte[4 + payload.Length];
            frame[0] = (byte)length;
            frame[1] = (byte)(length >> 8);
            frame[2] = (byte)(length >> 16);
            frame[3] = (byte)(length >> 24);
            Buffer.BlockCopy(payload, 0, frame, 4, payload.Length);
            return frame;
        }

        public static byte[] Decode(byte[] frame)
        {
            if (frame == null)
            {
                throw new ArgumentNullException(nameof(frame));
            }

            if (frame.Length < 4)
            {
                throw new InvalidDataException(
                    "Control frame is missing its four-byte length prefix.");
            }

            var declared = (uint)(frame[0]
                | frame[1] << 8
                | frame[2] << 16
                | frame[3] << 24);

            if (declared > MaxControlFrame)
            {
                throw new InvalidDataException(
                    $"Control frame payload is too large: {declared} bytes.");
            }

            var actual = frame.Length - 4;
            if (actual != declared)
            {
                throw new InvalidDataException(
                    $"Control frame length mismatch: declared {declared} bytes, received {actual}.");
            }

            var payload = new byte[actual];
            Buffer.BlockCopy(frame, 4, payload, 0, actual);
            return payload;
        }
    }
}
