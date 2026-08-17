import { QRCodeSVG } from 'qrcode.react';
import { useEffect, useState } from 'react';

interface QRDisplayProps {
    url: string | null;
    roomId: string | null;
}

const QRDisplay = ({ url, roomId: _roomId }: QRDisplayProps) => {
    const [displayUrl, setDisplayUrl] = useState<string>('');
    const [loading, setLoading] = useState(true);
    const [copied, setCopied] = useState(false);

    useEffect(() => {
        // `url` comes from usePeerHost and already carries the join token
        // (?t=...). Prefer it: a URL fetched from get_qr_url has no token, and
        // since signaling now verifies tokens on every join, a guest scanning
        // a tokenless QR would simply be rejected.
        if (url) {
            setDisplayUrl(url);
            setLoading(false);
            return;
        }

        // Never render a tokenless fallback QR. The server now verifies every
        // join, so a guest could load the page from such a URL but would be
        // rejected at Join Session.
        setDisplayUrl('');
        setLoading(true);
    }, [url]);

    const copyLink = async () => {
        try {
            await navigator.clipboard.writeText(displayUrl);
            setCopied(true);
            setTimeout(() => setCopied(false), 2000);
        } catch (err) {
            console.error('Failed to copy:', err);
        }
    };

    if (loading || !displayUrl) {
        return (
            <div style={{
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'center',
                padding: '24px',
                background: 'var(--bg-tertiary)',
                borderRadius: '12px'
            }}>
                <div style={{
                    width: '32px',
                    height: '32px',
                    border: '3px solid var(--bg-hover)',
                    borderTopColor: 'var(--accent)',
                    borderRadius: '50%',
                    animation: 'spin 1s linear infinite'
                }}></div>
                <p style={{ marginTop: '12px', color: 'var(--text-secondary)', fontSize: '14px' }}>
                    Loading...
                </p>
            </div>
        );
    }

    return (
        <div style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            gap: '16px'
        }}>
            {/* QR Code */}
            <div style={{
                background: 'white',
                padding: '16px',
                borderRadius: '12px',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center'
            }}>
                <QRCodeSVG
                    value={displayUrl}
                    size={180}
                    level="M"
                />
            </div>

            {/* URL Display */}
            <div style={{
                width: '100%',
                background: 'var(--bg-tertiary)',
                borderRadius: '8px',
                padding: '10px 12px',
                fontSize: '12px',
                fontFamily: 'monospace',
                color: 'var(--text-secondary)',
                textAlign: 'center',
                wordBreak: 'break-all'
            }}>
                {displayUrl}
            </div>

            {/* Copy Button */}
            <button
                onClick={copyLink}
                style={{
                    width: '100%',
                    padding: '10px 16px',
                    background: copied ? 'var(--success)' : 'var(--bg-tertiary)',
                    color: copied ? 'white' : 'var(--text-primary)',
                    border: 'none',
                    borderRadius: '8px',
                    cursor: 'pointer',
                    fontWeight: 500,
                    fontSize: '14px',
                    transition: 'all 0.2s'
                }}
            >
                {copied ? '✓ Copied!' : 'Copy Link'}
            </button>

            {/* Instructions */}
            <p style={{
                fontSize: '12px',
                color: 'var(--text-secondary)',
                textAlign: 'center',
                margin: 0
            }}>
                Scan QR code or share link to join
            </p>
        </div>
    );
};

export default QRDisplay;
