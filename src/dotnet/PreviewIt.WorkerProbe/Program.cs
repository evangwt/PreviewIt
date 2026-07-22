using System;
using System.IO;
using System.IO.Pipes;
using System.Threading.Tasks;
using Google.Protobuf;
using PreviewIt.Protocol;
using Previewit.Preview.V0;

namespace PreviewIt.WorkerProbe
{
    internal static class Program
    {
        private const int IoTimeoutMilliseconds = 5000;

        private static int Main(string[] args)
        {
            try
            {
                var options = Options.Parse(args);
                using (var pipe = new NamedPipeClientStream(
                    ".",
                    options.PipeName,
                    PipeDirection.InOut,
                    PipeOptions.Asynchronous))
                {
                    pipe.Connect(IoTimeoutMilliseconds);

                    if (options.SendOversizedFrame)
                    {
                        var declared = (uint)FramedProtocol.MaxControlFrame + 1;
                        WriteWithTimeout(pipe, new[]
                        {
                            (byte)declared,
                            (byte)(declared >> 8),
                            (byte)(declared >> 16),
                            (byte)(declared >> 24),
                        });
                        return 2;
                    }

                    var envelope = new Envelope
                    {
                        ProtocolMajor = options.ProtocolMajor,
                        ProtocolMinor = 1,
                        RequestId = "handshake-1",
                        Hello = new Hello { ComponentId = "dotnet-worker-probe" },
                    };
                    envelope.Hello.Capabilities.Add("read-handle-v0");
                    WriteWithTimeout(pipe, FramedProtocol.Encode(envelope.ToByteArray()));

                    var response = Envelope.Parser.ParseFrom(ReadFrame(pipe));
                    if (response.ProtocolMajor != 0 ||
                        response.ProtocolMinor != 1 ||
                        response.PayloadCase != Envelope.PayloadOneofCase.HelloAck ||
                        !response.HelloAck.AcceptedCapabilities.Contains("read-handle-v0"))
                    {
                        throw new InvalidDataException("Broker returned an invalid HelloAck.");
                    }
                }

                return 0;
            }
            catch (Exception error)
            {
                Console.Error.WriteLine(error.Message);
                return 1;
            }
        }

        private static byte[] ReadFrame(Stream stream)
        {
            var prefix = ReadExactWithTimeout(stream, 4);
            var declared = (uint)(prefix[0]
                | prefix[1] << 8
                | prefix[2] << 16
                | prefix[3] << 24);
            if (declared > FramedProtocol.MaxControlFrame)
            {
                throw new InvalidDataException("Broker declared an oversized control frame.");
            }

            var payload = ReadExactWithTimeout(stream, (int)declared);
            var frame = new byte[4 + payload.Length];
            Buffer.BlockCopy(prefix, 0, frame, 0, prefix.Length);
            Buffer.BlockCopy(payload, 0, frame, prefix.Length, payload.Length);
            return FramedProtocol.Decode(frame);
        }

        private static byte[] ReadExactWithTimeout(Stream stream, int length)
        {
            var buffer = new byte[length];
            var offset = 0;
            while (offset < buffer.Length)
            {
                var task = stream.ReadAsync(buffer, offset, buffer.Length - offset);
                if (!task.Wait(IoTimeoutMilliseconds))
                {
                    throw new TimeoutException("Pipe read timed out.");
                }

                var read = task.Result;
                if (read == 0)
                {
                    throw new EndOfStreamException("Pipe closed before a complete frame arrived.");
                }
                offset += read;
            }
            return buffer;
        }

        private static void WriteWithTimeout(Stream stream, byte[] buffer)
        {
            Task task = stream.WriteAsync(buffer, 0, buffer.Length);
            if (!task.Wait(IoTimeoutMilliseconds))
            {
                throw new TimeoutException("Pipe write timed out.");
            }
        }

        private sealed class Options
        {
            private Options(string pipeName, uint protocolMajor, bool sendOversizedFrame)
            {
                PipeName = pipeName;
                ProtocolMajor = protocolMajor;
                SendOversizedFrame = sendOversizedFrame;
            }

            public string PipeName { get; }

            public uint ProtocolMajor { get; }

            public bool SendOversizedFrame { get; }

            public static Options Parse(string[] args)
            {
                string pipeName = null;
                uint protocolMajor = 0;
                var oversized = false;

                for (var index = 0; index < args.Length; index++)
                {
                    switch (args[index])
                    {
                        case "--pipe" when index + 1 < args.Length:
                            pipeName = args[++index];
                            break;
                        case "--protocol-major" when index + 1 < args.Length:
                            protocolMajor = uint.Parse(args[++index]);
                            break;
                        case "--oversized":
                            oversized = true;
                            break;
                        default:
                            throw new ArgumentException($"Unknown or incomplete argument: {args[index]}");
                    }
                }

                if (string.IsNullOrWhiteSpace(pipeName) ||
                    pipeName.Contains("\\") ||
                    pipeName.Contains("/"))
                {
                    throw new ArgumentException("--pipe must be a bare named-pipe name.");
                }

                return new Options(pipeName, protocolMajor, oversized);
            }
        }
    }
}
