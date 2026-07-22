using System;
using System.Globalization;
using System.IO;
using System.IO.Pipes;
using System.Runtime.InteropServices;
using System.Threading.Tasks;
using Google.Protobuf;
using Microsoft.Win32.SafeHandles;
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

                    switch (options.Mode)
                    {
                        case WorkerMode.Handshake:
                            return 0;
                        case WorkerMode.Crash:
                            return 42;
                        case WorkerMode.Hang:
                            System.Threading.Thread.Sleep(System.Threading.Timeout.Infinite);
                            return 1;
                        case WorkerMode.Handles:
                            RunRequests(pipe, stale: false);
                            return 0;
                        case WorkerMode.Stale:
                            RunRequests(pipe, stale: true);
                            return 0;
                        default:
                            throw new InvalidDataException("Unknown worker mode.");
                    }
                }
            }
            catch (Exception error)
            {
                Console.Error.WriteLine(error.Message);
                return 1;
            }
        }

        private static void RunRequests(NamedPipeClientStream pipe, bool stale)
        {
            Envelope previousResult = null;
            while (true)
            {
                var request = Envelope.Parser.ParseFrom(ReadFrame(pipe));
                switch (request.PayloadCase)
                {
                    case Envelope.PayloadOneofCase.Cancel:
                        return;
                    case Envelope.PayloadOneofCase.OpenDocument:
                        if (stale && previousResult != null)
                        {
                            WriteWithTimeout(pipe, FramedProtocol.Encode(previousResult.ToByteArray()));
                        }

                        var (result, writeError) = ReadDocument(request);
                        WriteWithTimeout(pipe, FramedProtocol.Encode(result.ToByteArray()));
                        WriteWithTimeout(pipe, FramedProtocol.Encode(writeError.ToByteArray()));
                        previousResult = result.Clone();
                        break;
                    default:
                        throw new InvalidDataException("Worker received an unsupported request.");
                }
            }
        }

        private static (Envelope Result, Envelope WriteError) ReadDocument(Envelope request)
        {
            var document = request.OpenDocument;
            if (document.Size > int.MaxValue)
            {
                throw new InvalidDataException("The foundation worker only accepts int-sized fixtures.");
            }

            var rawHandle = new IntPtr(unchecked((long)document.DuplicatedHandle));
            using (var safeHandle = new SafeFileHandle(rawHandle, ownsHandle: true))
            {
                var payload = ReadFromHandle(safeHandle, (int)document.Size);
                var result = new Envelope
                {
                    ProtocolMajor = 0,
                    ProtocolMinor = 1,
                    RequestId = request.RequestId,
                    Result = new Result
                    {
                        Status = "read-ok",
                        Payload = ByteString.CopyFrom(payload),
                    },
                };

                var writeError = 0;
                var writeSucceeded = TryWriteOneByte(safeHandle, out writeError);
                var error = new Envelope
                {
                    ProtocolMajor = 0,
                    ProtocolMinor = 1,
                    RequestId = request.RequestId,
                    Error = new PreviewError
                    {
                        Code = writeSucceeded ? "write-succeeded" : "write-denied",
                        Message = writeError.ToString(CultureInfo.InvariantCulture),
                    },
                };
                return (result, error);
            }
        }

        private static byte[] ReadFromHandle(SafeFileHandle handle, int length)
        {
            var payload = new byte[length];
            var offset = 0;
            while (offset < payload.Length)
            {
                var chunk = new byte[Math.Min(payload.Length - offset, 64 * 1024)];
                uint read;
                if (!ReadFile(
                    handle,
                    chunk,
                    (uint)chunk.Length,
                    out read,
                    IntPtr.Zero))
                {
                    throw new IOException($"ReadFile failed: {Marshal.GetLastWin32Error()}");
                }

                if (read == 0)
                {
                    throw new EndOfStreamException("Duplicated handle ended before the advertised size.");
                }
                Buffer.BlockCopy(chunk, 0, payload, offset, checked((int)read));
                offset += checked((int)read);
            }
            return payload;
        }

        private static bool TryWriteOneByte(SafeFileHandle handle, out int error)
        {
            var byteToWrite = new byte[] { 0x58 };
            uint written;
            var succeeded = WriteFile(handle, byteToWrite, 1, out written, IntPtr.Zero);
            error = succeeded ? 0 : Marshal.GetLastWin32Error();
            return succeeded && written == 1;
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
            private Options(string pipeName, uint protocolMajor, bool sendOversizedFrame, WorkerMode mode)
            {
                PipeName = pipeName;
                ProtocolMajor = protocolMajor;
                SendOversizedFrame = sendOversizedFrame;
                Mode = mode;
            }

            public string PipeName { get; }

            public uint ProtocolMajor { get; }

            public bool SendOversizedFrame { get; }

            public WorkerMode Mode { get; }

            public static Options Parse(string[] args)
            {
                string pipeName = null;
                uint protocolMajor = 0;
                var oversized = false;
                var mode = WorkerMode.Handshake;

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
                        case "--mode" when index + 1 < args.Length:
                            mode = ParseMode(args[++index]);
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

                return new Options(pipeName, protocolMajor, oversized, mode);
            }

            private static WorkerMode ParseMode(string value)
            {
                switch (value)
                {
                    case "handshake": return WorkerMode.Handshake;
                    case "handles": return WorkerMode.Handles;
                    case "crash": return WorkerMode.Crash;
                    case "hang": return WorkerMode.Hang;
                    case "stale": return WorkerMode.Stale;
                    default: throw new ArgumentException($"Unknown worker mode: {value}");
                }
            }
        }

        private enum WorkerMode
        {
            Handshake,
            Handles,
            Crash,
            Hang,
            Stale,
        }

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool ReadFile(
            SafeFileHandle hFile,
            [Out] byte[] lpBuffer,
            uint nNumberOfBytesToRead,
            out uint lpNumberOfBytesRead,
            IntPtr lpOverlapped);

        [DllImport("kernel32.dll", SetLastError = true)]
        private static extern bool WriteFile(
            SafeFileHandle hFile,
            byte[] lpBuffer,
            uint nNumberOfBytesToWrite,
            out uint lpNumberOfBytesWritten,
            IntPtr lpOverlapped);
    }
}
