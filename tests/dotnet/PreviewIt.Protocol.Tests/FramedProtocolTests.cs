using System.IO;
using System.Text;
using Google.Protobuf;
using Microsoft.VisualStudio.TestTools.UnitTesting;
using Previewit.Preview.V0;

namespace PreviewIt.Protocol.Tests
{
    [TestClass]
    public sealed class FramedProtocolTests
    {
        [TestMethod]
        public void FrameRoundTrips()
        {
            var payload = Encoding.UTF8.GetBytes("previewit");
            var frame = FramedProtocol.Encode(payload);

            CollectionAssert.AreEqual(payload, FramedProtocol.Decode(frame));
        }

        [TestMethod]
        public void OversizedControlFrameIsRejected()
        {
            Assert.ThrowsExactly<InvalidDataException>(() =>
                FramedProtocol.Encode(new byte[FramedProtocol.MaxControlFrame + 1]));
        }

        [TestMethod]
        public void TruncatedFrameIsRejected()
        {
            Assert.ThrowsExactly<InvalidDataException>(() =>
                FramedProtocol.Decode(new byte[] { 4, 0, 0, 0, 1, 2 }));
        }

        [TestMethod]
        public void ProtobufEnvelopeRoundTrips()
        {
            var envelope = new Envelope
            {
                ProtocolMajor = 0,
                ProtocolMinor = 1,
                RequestId = "request-1",
                Hello = new Hello
                {
                    ComponentId = "dotnet-probe"
                }
            };
            envelope.Hello.Capabilities.Add("read-handle-v0");

            var parsed = Envelope.Parser.ParseFrom(envelope.ToByteArray());

            Assert.AreEqual(0U, parsed.ProtocolMajor);
            Assert.AreEqual(1U, parsed.ProtocolMinor);
            Assert.AreEqual("request-1", parsed.RequestId);
            Assert.AreEqual(Envelope.PayloadOneofCase.Hello, parsed.PayloadCase);
            Assert.AreEqual("dotnet-probe", parsed.Hello.ComponentId);
            Assert.AreEqual(1, parsed.Hello.Capabilities.Count);
            Assert.AreEqual("read-handle-v0", parsed.Hello.Capabilities[0]);
        }
    }
}
