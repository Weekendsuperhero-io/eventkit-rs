---
created: 2026-05-29T04:27
updated: 2026-05-29T04:27
---
```
@interface EKCalendar : EKObject
```

## Overview

Use the properties in this class to get attributes about a calendar, such as its title and type. Use the [`calendarForEntityType:eventStore:`](https://developer.apple.com/documentation/eventkit/ekcalendar/init\(for:eventstore:\)?language=objc) method to create a calendar object.

## Topics

### Creating Calendars

[`+ calendarForEntityType:eventStore:`](https://developer.apple.com/documentation/eventkit/ekcalendar/init\(for:eventstore:\)?language=objc)

Creates a new calendar that can contain the given entity type.

[`+ calendarWithEventStore:`](https://developer.apple.com/documentation/eventkit/ekcalendar/init\(eventstore:\)?language=objc)

Creates and returns a calendar belonging to a specified event store.

Deprecated

### Accessing Calendar Properties

[`EKCalendarType`](https://developer.apple.com/documentation/eventkit/ekcalendartype?language=objc)

Possible calendar types.

[`EKCalendarEventAvailabilityMask`](https://developer.apple.com/documentation/eventkit/ekcalendareventavailabilitymask?language=objc)

A bitmask indicating the event availability settings that the calendar can support.

[`allowsContentModifications`](https://developer.apple.com/documentation/eventkit/ekcalendar/allowscontentmodifications?language=objc)

A Boolean value that indicates whether you can add, edit, and delete items in the calendar.

[`CGColor`](https://developer.apple.com/documentation/eventkit/ekcalendar/cgcolor?language=objc)

The calendar’s color.

[`color`](https://developer.apple.com/documentation/eventkit/ekcalendar/color?language=objc)

The calendar’s color.

[`immutable`](https://developer.apple.com/documentation/eventkit/ekcalendar/isimmutable?language=objc)

A Boolean value indicating whether the calendar’s properties can be edited or deleted.

[`title`](https://developer.apple.com/documentation/eventkit/ekcalendar/title?language=objc)

The calendar’s title.

[`type`](https://developer.apple.com/documentation/eventkit/ekcalendar/type?language=objc)

The calendar’s type.

[`allowedEntityTypes`](https://developer.apple.com/documentation/eventkit/ekcalendar/allowedentitytypes?language=objc)

The entity types this calendar can contain.

[`source`](https://developer.apple.com/documentation/eventkit/ekcalendar/source?language=objc)

The source object representing the account to which this calendar belongs.

[`subscribed`](https://developer.apple.com/documentation/eventkit/ekcalendar/issubscribed?language=objc)

A Boolean value indicating whether the calendar is a subscribed calendar.

[`supportedEventAvailabilities`](https://developer.apple.com/documentation/eventkit/ekcalendar/supportedeventavailabilities?language=objc)

The event availability settings supported by this calendar, as indicated by a bitmask.

[`calendarIdentifier`](https://developer.apple.com/documentation/eventkit/ekcalendar/calendaridentifier?language=objc)

A unique identifier for the calendar.

[`DATETIME_COMPONENTS_DO_NOT_USE`](https://developer.apple.com/documentation/eventkit/datetime_components_do_not_use\(\)?language=objc)

A deprecated function.

Deprecated

[`DATE_COMPONENTS_DO_NOT_USE`](https://developer.apple.com/documentation/eventkit/date_components_do_not_use\(\)?language=objc)

A deprecated function.

Deprecated

## Relationships

- [`EKObject`](https://developer.apple.com/documentation/eventkit/ekobject?language=objc)