using System.Diagnostics;
using System.Net;
using System.Net.Sockets;

namespace Kilohertz.DofBridge;

/// <summary>
/// Owns the UDP socket and a dedicated receive thread. Each datagram is stamped with
/// <see cref="Stopwatch.GetTimestamp"/> right after <c>Receive</c> returns and handed to
/// <see cref="BridgeCore.Process"/> on that same thread.
/// </summary>
public sealed class UdpBridgeHost : IDisposable
{
    private const int ReceiveBufferBytes = 1 << 20;
    private readonly BridgeCore _core;
    private readonly Socket _socket;
    private Thread? _thread;
    private long _received;
    private volatile bool _stopping;

    /// <summary>Binds the socket (port 0 picks a free port; see <see cref="LocalEndPoint"/>).</summary>
    /// <param name="core">The core to feed.</param>
    /// <param name="bind">Local address and port.</param>
    /// <exception cref="SocketException">The address cannot be bound.</exception>
    public UdpBridgeHost(BridgeCore core, IPEndPoint bind)
    {
        ArgumentNullException.ThrowIfNull(core);
        ArgumentNullException.ThrowIfNull(bind);
        _core = core;
        _socket = new Socket(bind.AddressFamily, SocketType.Dgram, ProtocolType.Udp);
        try
        {
            _socket.ReceiveBufferSize = ReceiveBufferBytes;
            _socket.Bind(bind);
        }
        catch
        {
            _socket.Dispose();
            throw;
        }
    }

    /// <summary>The bound local endpoint.</summary>
    public IPEndPoint LocalEndPoint => (IPEndPoint)_socket.LocalEndPoint!;

    /// <summary>Datagrams received and processed so far (incremented after <see cref="BridgeCore.Process"/>).</summary>
    public long Received => Interlocked.Read(ref _received);

    /// <summary>Warms up the datagram path and starts the receive thread.</summary>
    public void Start()
    {
        if (_thread != null)
        {
            return;
        }
        BridgeCore.Warmup();
        _thread = new Thread(Loop)
        {
            IsBackground = true,
            Name = "dof-bridge-udp",
            Priority = ThreadPriority.AboveNormal,
        };
        _thread.Start();
    }

    private void Loop()
    {
        // One byte larger than the largest valid datagram so an oversized one is seen (and rejected).
        byte[] buffer = new byte[TelemetryParser.MaxDatagramLength + 1];
        while (!_stopping)
        {
            int n;
            try
            {
                n = _socket.Receive(buffer);
            }
            catch (SocketException e) when (!_stopping && (e.SocketErrorCode is SocketError.ConnectionReset or SocketError.MessageSize))
            {
                // Windows reports ICMP port-unreachable and oversized datagrams this way; skip them.
                continue;
            }
            catch (Exception e) when (e is SocketException or ObjectDisposedException)
            {
                return;
            }
            long stamp = Stopwatch.GetTimestamp();
            if (_stopping)
            {
                // A shut-down socket returns 0 bytes on some platforms: not a datagram.
                return;
            }
            _core.Process(buffer.AsSpan(0, n), stamp);
            Interlocked.Increment(ref _received);
        }
    }

    /// <summary>Closes the socket and joins the receive thread.</summary>
    public void Stop()
    {
        if (_stopping)
        {
            return;
        }
        _stopping = true;
        try
        {
            _socket.Shutdown(SocketShutdown.Both);
        }
        catch (SocketException)
        {
            // Not connected: fine for UDP.
        }
        _socket.Close();
        _thread?.Join();
    }

    /// <inheritdoc />
    public void Dispose()
    {
        Stop();
        _socket.Dispose();
    }
}
