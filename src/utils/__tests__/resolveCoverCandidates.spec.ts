import { describe, expect, it } from 'vitest'
import { resolveCoverCandidates, resolveCoverImage } from '../image'

const video = {
  poster: '/v/a-poster.jpg',
  thumb: '/v/a-thumb.jpg',
  fanart: '/v/a-fanart.jpg',
  coverThumb: '/v/a-thumbsm.jpg',
}

describe('resolveCoverCandidates', () => {
  it('横屏默认：fanart → thumb → poster，不带小缩略图', () => {
    expect(resolveCoverCandidates(video, 'landscape')).toEqual([
      '/v/a-fanart.jpg',
      '/v/a-thumb.jpg',
      '/v/a-poster.jpg',
    ])
  })

  it('横屏优先小缩略图时把 coverThumb 放在最前', () => {
    expect(resolveCoverCandidates(video, 'landscape', true)[0]).toBe('/v/a-thumbsm.jpg')
  })

  it('竖屏：poster → fanart → thumb，且忽略小缩略图', () => {
    expect(resolveCoverCandidates(video, 'portrait', true)).toEqual([
      '/v/a-poster.jpg',
      '/v/a-fanart.jpg',
      '/v/a-thumb.jpg',
    ])
  })

  it('去空、去重', () => {
    expect(resolveCoverCandidates({ poster: '/x.jpg', thumb: '/x.jpg', fanart: '' }, 'landscape')).toEqual(['/x.jpg'])
    expect(resolveCoverCandidates({}, 'landscape', true)).toEqual([])
  })

  it('resolveCoverImage 即候选首项', () => {
    expect(resolveCoverImage(video, 'portrait')).toBe('/v/a-poster.jpg')
    expect(resolveCoverImage({}, 'landscape')).toBeUndefined()
  })
})
