import { describe, expect, it } from 'vitest';
import { youtubeShortUrl } from '../youtube-url';

describe('youtubeShortUrl', () => {
    it('builds the shareable youtu.be URL for a video id', () => {
        expect(youtubeShortUrl('abc123')).toBe('https://youtu.be/abc123');
    });
});
