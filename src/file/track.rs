use crate::byte_iter::ByteIter;
use crate::core::{
    Channel, Clocks, DurationName, GeneralMidi, Message, NoteMessage, NoteNumber, Program,
    ProgramChangeValue, Velocity,
};
use crate::error::LibResult;
use crate::file::{
    Event, MetaEvent, MicrosecondsPerQuarter, QuartersPerMinute, TimeSignatureValue, TrackEvent,
};
use crate::scribe::{Scribe, ScribeSettings};
use crate::Text;
use log::{debug, trace};
use snafu::ResultExt;
use std::convert::TryFrom;
use std::io::{Read, Write};

#[derive(Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Hash)]
pub struct Track {
    events: Vec<TrackEvent>,
}

impl Track {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn events_len(&self) -> usize {
        self.events.len()
    }

    pub fn events(&self) -> impl Iterator<Item = &TrackEvent> {
        self.events.iter()
    }

    pub fn push_event(&mut self, delta_time: u32, event: Event) -> crate::Result<()> {
        // TODO check length is not bigger than u32
        self.events.push(TrackEvent::new(delta_time, event));
        Ok(())
    }

    pub fn insert_event(&mut self, index: u32, delta_time: u32, event: Event) -> crate::Result<()> {
        // TODO check length is not bigger than u32, index is in range, etc
        self.events
            .insert(index as usize, TrackEvent::new(delta_time, event));
        Ok(())
    }

    pub fn replace_event(
        &mut self,
        index: u32,
        delta_time: u32,
        event: Event,
    ) -> crate::Result<()> {
        // TODO check length is not bigger than u32, index is in range, etc
        // std::mem::replace(&mut , TrackEvent{delta_time, event})
        self.events[index as usize] = TrackEvent::new(delta_time, event);
        Ok(())
    }

    pub fn set_name<S: Into<String>>(&mut self, name: S) -> crate::Result<()> {
        let name = Text::new(name);
        let meta = Event::Meta(MetaEvent::TrackName(name.clone()));
        if self.is_empty() {
            self.push_event(0, meta)?;
            return Ok(());
        }
        for (ix, event) in self.events.iter_mut().enumerate() {
            if event.delta_time() != 0 {
                break;
            }
            if let Event::Meta(MetaEvent::TrackName(s)) = event.event() {
                debug!("changing track name from '{}' to '{}'", s, name);
                self.replace_event(ix as u32, 0, meta)?;
                return Ok(());
            }
        }
        self.insert_event(0, 0, meta)?;
        Ok(())
    }

    pub fn set_instrument_name<S: Into<String>>(&mut self, name: S) -> crate::Result<()> {
        let name = Text::new(name);
        let meta = Event::Meta(MetaEvent::InstrumentName(name.clone()));
        if self.is_empty() {
            self.push_event(0, meta)?;
            return Ok(());
        }
        for (ix, event) in self.events.iter_mut().enumerate() {
            if event.delta_time() != 0 {
                break;
            }
            if let Event::Meta(MetaEvent::InstrumentName(s)) = event.event() {
                debug!("changing instrument name from '{}' to '{}'", s, name);
                self.replace_event(ix as u32, 0, meta)?;
                return Ok(());
            }
        }
        self.insert_event(0, 0, meta)?;
        Ok(())
    }

    pub fn set_general_midi(&mut self, channel: Channel, value: GeneralMidi) -> crate::Result<()> {
        let program_change = Event::Midi(Message::ProgramChange(ProgramChangeValue {
            channel,
            program: Program::new(value.into()),
        }));
        if self.is_empty() {
            self.push_event(0, program_change)?;
            return Ok(());
        }
        for (ix, event) in self.events.iter_mut().enumerate() {
            if event.delta_time() != 0 {
                break;
            }
            if let Event::Midi(Message::ProgramChange(prog)) = event.event() {
                debug!(
                    "changing program from '{}' to '{:?}'",
                    prog.program.get(),
                    value
                );
                self.replace_event(ix as u32, 0, program_change)?;
                return Ok(());
            }
        }
        self.insert_event(0, 0, program_change)?;
        Ok(())
    }

    pub fn push_time_signature(
        &mut self,
        delta_time: u32,
        numerator: u8,
        denominator: DurationName,
        click: Clocks,
    ) -> crate::Result<()> {
        let time_sig = TimeSignatureValue::new(numerator, denominator, click)?;
        let event = Event::Meta(MetaEvent::TimeSignature(time_sig));
        self.push_event(delta_time, event)
    }

    pub fn push_tempo(
        &mut self,
        delta_time: u32,
        quarters_per_minute: QuartersPerMinute,
    ) -> crate::Result<()> {
        // convert to microseconds per quarter note
        let minutes_per_quarter = 1f64 / f64::from(quarters_per_minute.get());
        let seconds_per_quarter = minutes_per_quarter * 60f64;
        let microseconds_per_quarter = seconds_per_quarter * 1000000f64;
        let value = microseconds_per_quarter as u32;
        let event = Event::Meta(MetaEvent::SetTempo(MicrosecondsPerQuarter::new(value)));
        self.push_event(delta_time, event)
    }

    pub fn push_note_on(
        &mut self,
        delta_time: u32,
        channel: Channel,
        note_number: NoteNumber,
        velocity: Velocity,
    ) -> crate::Result<()> {
        let note_on = Event::Midi(Message::NoteOn(NoteMessage {
            channel,
            note_number,
            velocity,
        }));
        self.push_event(delta_time, note_on)?;
        Ok(())
    }

    pub fn push_note_off(
        &mut self,
        delta_time: u32,
        channel: Channel,
        note_number: NoteNumber,
        velocity: Velocity,
    ) -> crate::Result<()> {
        let note_off = Event::Midi(Message::NoteOff(NoteMessage {
            channel,
            note_number,
            velocity,
        }));
        self.push_event(delta_time, note_off)
    }

    pub fn push_lyric<S: Into<String>>(&mut self, delta_time: u32, lyric: S) -> crate::Result<()> {
        let lyric = Event::Meta(MetaEvent::Lyric(Text::new(lyric)));
        self.push_event(delta_time, lyric)
    }

    pub(crate) fn parse<R: Read>(iter: &mut ByteIter<R>) -> LibResult<Self> {
        iter.expect_tag("MTrk").context(io!())?;
        let chunk_length = iter.read_u32().context(io!())?;
        iter.set_size_limit(chunk_length as u64);
        let mut events = Vec::new();
        loop {
            if iter.is_end() {
                invalid_file!("end of track bytes reached before EndOfTrack event.");
            }
            let event = TrackEvent::parse(iter)?;
            trace!("parsed {:?}", event);
            let is_track_end = event.is_end();
            events.push(event);
            if is_track_end {
                debug!("end of track event");
                if !iter.is_end() {
                    invalid_file!("EndOfTrack event before end of track bytes.");
                }
                break;
            }
        }
        iter.clear_size_limit();
        Ok(Self { events })
    }

    pub(crate) fn write<W: Write>(&self, w: &mut Scribe<W>) -> LibResult<()> {
        // write the track chunk header
        w.write_all(b"MTrk").context(wr!())?;

        // we need to write out all of the data first so we know its length
        let mut track_data: Vec<u8> = Vec::new();
        let mut track_scribe = Scribe::new(&mut track_data, ScribeSettings::default());
        for event in self.events() {
            event.write(&mut track_scribe)?;
        }

        // write the length of the track
        let track_length = u32::try_from(track_data.len())
            .context(crate::error::TrackTooLong { site: site!() })?;
        w.write_all(&track_length.to_be_bytes()).context(wr!())?;

        // write the track data
        w.write_all(&track_data).context(wr!())?;
        Ok(())
    }
}

/// If the last item of the track is *not* an end-of-track event, then add it to the back. If
/// the track already has an end-of-track event as its last event, then nothing happens.
pub(crate) fn ensure_end_of_track(mut track: Track) -> LibResult<Track> {
    if let Some(last_event) = track.events.last() {
        if !matches!(last_event.event(), Event::Meta(MetaEvent::EndOfTrack)) {
            track.push_event(0, Event::Meta(MetaEvent::EndOfTrack))?;
        }
    } else {
        track.push_event(0, Event::Meta(MetaEvent::EndOfTrack))?;
    }
    Ok(track)
}
