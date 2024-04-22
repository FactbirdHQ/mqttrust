
// impl<S: Read + Write> Transport for S {
//     type Socket = S;

//     async fn connect(&mut self, _broker: &mut impl Broker) -> Result<(), ConnectionError> {
//         Ok(())
//     }

//     fn disconnect(&mut self) -> Result<(), ConnectionError> {
//         Ok(())
//     }

//     fn is_connected(&self) -> bool {
//         true
//     }

//     async fn write_packet<P: MqttEncode>(&mut self, packet: P) -> Result<(), StateError> {
//         // FIXME: Reuse packet buffer?
//         let mut buf = [0u8; 128];
//         let mut encoder = MqttEncoder::new(&mut buf);
//         packet
//             .to_buffer(&mut encoder)
//             .map_err(|_| StateError::Deserialization)?;
//         Transport::write(self, encoder.packet_bytes()).await?;
//         Ok(())
//     }

//     async fn write(&mut self, buf: &[u8]) -> Result<(), StateError> {
//         trace!("Writing {} bytes to socket", buf.len());
//         self.write_all(buf)
//             .await
//             .map_err(|e| StateError::Io(e.kind()))?;
//         self.flush().await.map_err(|e| StateError::Io(e.kind()))
//     }

//     async fn get_received_packet(
//         &mut self,
//     ) -> Result<ReceivedPacket<'_, Self::Socket>, StateError> {
//         if !self.is_connected() {
//             return Err(StateError::Io(ErrorKind::NotConnected));
//         };

//         while !self.packet_buf.packet_available() {
//             self.packet_buf
//                 .receive(self.socket.as_mut().unwrap())
//                 .await
//                 .map_err(|kind| {
//                     error!("DISCONNECTING {:?}", kind);
//                     self.socket.take();
//                     StateError::Io(kind)
//                 })?;
//         }

//         self.packet_buf
//             .received_packet(self.socket.as_mut().unwrap())
//             .map_err(|_| StateError::Deserialization)
//     }
// }
